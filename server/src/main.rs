pub mod mpvplayer;
use anyhow::{anyhow, Result};
use core::time;
use std::{fmt::Debug, process::ExitCode, time::Duration};

use clap::Parser;
use mpvplayer::*;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    source: Option<String>,

    #[arg(short, long, value_parser = clap::value_parser!(u8).range(0..100))]
    volume: Option<usize>,

    #[arg(short, long)]
    #[arg(value_parser = parse_duration)]
    at: Option<time::Duration>,

    #[arg(short, long, action)]
    daemon: bool,
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

async fn execute() -> Result<()> {
    let args = Args::try_parse()?;
    let daemon = args.daemon;
    let source = args.source;
    let volume = args.volume;
    let at = args.at;

    if source.is_none() && daemon == false {
        return Err(anyhow!(
            "Invalid Arguments: Must either give --daemon or --source arguments"
        ));
    } else if source.is_none() && (volume.is_some() || at.is_some()) {
        return Err(anyhow!(
            "Invalid Arguments: Volume --volume and --at arguments require the --source argument"
        ));
    }

    let player = MpvPlayer::new(daemon).await?;

    if !daemon {
        if let Ok(player) = player.write() {
            if let Some(source) = source {
                let loaded = player.queue_request(comms::Request::load(source)).await;
                if !loaded {
                    return Err(anyhow!("Unable to queue load request"));
                }
                if let Some(at) = at {
                    let seek = player
                        .queue_request(comms::Request::seek(at, comms::Units::Seconds))
                        .await;
                    if !seek {
                        return Err(anyhow!("Unable to queue seek request"));
                    }
                }

                if let Some(volume) = volume {
                    let volume = player
                        .queue_request(comms::Request::volume(volume as i64))
                        .await;
                    if !volume {
                        return Err(anyhow!("Unable to queue volume request"));
                    }
                }
            }
        }
    }

    // Wait until the player has exited
    loop {
        if let Ok(player) = player.try_read() {
            match player.player_state() {
                PlayerState::Exiting => {
                    println!("Player exiting");
                    break;
                }
                _ => (),
            }
        }

        async_std::task::sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}

#[async_std::main]
async fn main() -> ExitCode {
    match execute().await {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            println!("{:?}", e);
            ExitCode::FAILURE
        }
    }
}
