use std::error::Error;
use std::fs::{File, Metadata};
use std::io::Read;
use std::path::PathBuf;
use std::thread::spawn;

use clap::{command, value_parser, Arg};
use crossbeam_channel::{bounded, Sender};
use glob::glob;
use rayon::{
    iter::{IntoParallelIterator, ParallelBridge, ParallelIterator},
    ThreadPoolBuilder,
};
use tar::{Builder, Header};
use zstd::Encoder;

fn main() -> Fallible {
    let matches = command!()
        .arg(
            Arg::new("OUTPUT")
                .required(true)
                .value_parser(value_parser!(PathBuf)),
        )
        .arg(Arg::new("INPUTS").required(true).num_args(1..))
        .arg(
            Arg::new("JOBS")
                .short('j')
                .long("jobs")
                .default_value("1")
                .value_parser(value_parser!(usize)),
        )
        .arg(
            Arg::new("LEVEL")
                .short('l')
                .long("level")
                .default_value("0")
                .value_parser(value_parser!(i32)),
        )
        .arg(
            Arg::new("WORKERS")
                .short('w')
                .long("workers")
                .default_value("1")
                .value_parser(value_parser!(u32)),
        )
        .get_matches();

    let output = matches.get_one::<PathBuf>("OUTPUT").unwrap();
    let inputs = matches
        .get_many::<String>("INPUTS")
        .unwrap()
        .map(|inputs| inputs.to_owned())
        .collect::<Vec<_>>();

    let jobs = *matches.get_one::<usize>("JOBS").unwrap();
    let level = *matches.get_one::<i32>("LEVEL").unwrap();
    let workers = *matches.get_one::<u32>("WORKERS").unwrap();

    ThreadPoolBuilder::new().num_threads(jobs).build_global()?;

    let (buffers_sender, buffers_receiver) = bounded(jobs);

    fn read_file(path: PathBuf) -> Fallible<(PathBuf, Metadata, Vec<u8>)> {
        let mut file = File::open(&path)?;

        let metadata = file.metadata()?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        Ok((path, metadata, buffer))
    }

    fn read_dir(buffers_sender: &Sender<(PathBuf, Metadata, Vec<u8>)>, dir: PathBuf) -> Fallible {
        dir.read_dir()?.par_bridge().try_for_each(|entry| {
            let entry = entry?;
            let path = entry.path();

            if entry.file_type()?.is_dir() {
                read_dir(buffers_sender, path)?;
            } else {
                let buffer = read_file(path)?;
                buffers_sender.send(buffer).unwrap();
            }

            Ok(())
        })
    }

    let reader = spawn(move || -> Fallible {
        inputs.into_par_iter().try_for_each(|inputs| {
            glob(&inputs)?.par_bridge().try_for_each(|input| {
                let path = input?;

                if path.is_dir() {
                    read_dir(&buffers_sender, path)?;
                } else {
                    let buffer = read_file(path)?;
                    buffers_sender.send(buffer).unwrap();
                }

                Ok(())
            })
        })
    });

    let mut encoder = Encoder::new(File::create(output)?, level)?;
    encoder.multithread(workers)?;
    let mut builder = Builder::new(encoder);

    for (path, metadata, buffer) in buffers_receiver {
        eprintln!("{}", path.display());

        let mut header = Header::new_gnu();
        header.set_metadata(&metadata);
        builder.append_data(&mut header, path, &*buffer).unwrap();
    }

    let encoder = builder.into_inner()?;
    encoder.finish()?;

    reader.join().unwrap()?;

    Ok(())
}

type Fallible<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
