pub mod mpvplayer;
use anyhow::Result;
use std::{process::ExitCode, time::Duration};

use mpvplayer::*;

async fn execute() -> Result<()> {
    let player = MpvPlayer::new().await?;

    // Wait until the player has exited
    loop {
        match player.player_state() {
            PlayerState::Exiting => {
                println!("Player exiting");
                break;
            }
            _ => (),
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
