use std::error::Error;
use std::fs::{read, File};
use std::thread::spawn;

use clap::{crate_authors, crate_name, crate_version, App, Arg};
use crossbeam_channel::{bounded, unbounded};
use glob::glob;
use tar::{Builder, Header};
use zstd::Encoder;

fn main() -> Fallible {
    let matches = App::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!(", "))
        .arg(Arg::with_name("OUTPUT").required(true))
        .arg(Arg::with_name("INPUTS").required(true).multiple(true))
        .arg(
            Arg::with_name("JOBS")
                .short("j")
                .long("jobs")
                .default_value("1"),
        )
        .get_matches();

    let output = matches.value_of("OUTPUT").unwrap();
    let inputs = matches.values_of("INPUTS").unwrap();
    let jobs = matches.value_of("JOBS").unwrap().parse::<usize>()?;

    let (inputs_sender, inputs_receiver) = unbounded();
    let (buffers_sender, buffers_receiver) = bounded(jobs);

    let mut jobs = (0..jobs)
        .map(move |_| {
            let inputs_receiver = inputs_receiver.clone();
            let buffers_sender = buffers_sender.clone();

            spawn(move || -> Fallible {
                for input in inputs_receiver {
                    let buffer = read(&input)?;

                    buffers_sender.send((input, buffer)).unwrap();
                }

                Ok(())
            })
        })
        .collect::<Vec<_>>();

    jobs.extend(inputs.map(move |inputs| {
        let inputs = inputs.to_owned();
        let inputs_sender = inputs_sender.clone();

        spawn(move || {
            for input in glob(&inputs)? {
                let input = input?;

                inputs_sender.send(input).unwrap();
            }

            Ok(())
        })
    }));

    let mut builder = Builder::new(Encoder::new(File::create(output)?, 0)?);

    for (path, buffer) in buffers_receiver {
        eprintln!("{}", path.display());

        let mut header = Header::new_gnu();
        header.set_path(path)?;
        header.set_size(buffer.len() as u64);
        header.set_cksum();

        builder.append(&header, &*buffer).unwrap();
    }

    let encoder = builder.into_inner()?;
    encoder.finish()?;

    for job in jobs {
        job.join().unwrap()?;
    }

    Ok(())
}

type Fallible<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
