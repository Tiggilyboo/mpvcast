pub mod mpvplayer;
use anyhow::Result;
use core::time;
use std::process::ExitCode;

use clap::Parser;
use mpvplayer::MpvPlayer;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    source: String,

    #[arg(short, long, value_parser = clap::value_parser!(u8).range(0..100))]
    volume: Option<usize>,

    #[arg(short, long)]
    #[arg(value_parser = parse_duration)]
    at: Option<time::Duration>,
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

fn execute() -> Result<()> {
    let args = Args::try_parse()?;
    let source = args.source;
    let volume = args.volume.unwrap_or(80) as i64;
    let at = args.at.unwrap_or(time::Duration::ZERO);

    let player = MpvPlayer::new()?;
    player.stream_video(&source, volume, at)?;

    Ok(())
}

fn main() -> ExitCode {
    match execute() {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
