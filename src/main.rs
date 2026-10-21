mod cli;
mod filesystem;
mod mount;

use std::error::Error;
use std::process::ExitCode;

use clap::Parser;
use easy_fuser::fuse_parallel::prelude::MountOption;
use log::{LevelFilter, info};

use crate::cli::Args;
use crate::filesystem::AcFs;

fn main() -> ExitCode {
    let args = Args::parse();
    init_logging(args.verbose);

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            log::error!("{err}");
            eprintln!("vcs-objectfs: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<(), Box<dyn Error>> {
    let source = args.resolve_source()?;
    let mountpoint = args.resolve_mountpoint(&source)?;

    let mut options = vec![
        MountOption::FSName("vcs-objectfs".to_string()),
        MountOption::Subtype("vcs-objectfs".to_string()),
    ];
    if args.read_only {
        options.push(MountOption::RO);
    }
    options.extend(
        args.mount_options
            .iter()
            .map(|option| MountOption::CUSTOM(option.clone())),
    );

    info!("serving {} at {}", source.display(), mountpoint.display());

    let fs = AcFs::new(source);
    mount::run(fs, &mountpoint, &options)?;

    info!("unmounted {}", mountpoint.display());
    Ok(())
}

fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level.as_str()))
        .format_timestamp_millis()
        .init();
}
