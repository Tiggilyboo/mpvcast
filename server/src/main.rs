pub mod mpvplayer;
use anyhow::Result;
use std::process::ExitCode;

use mpvplayer::*;

async fn execute() -> Result<()> {
    MpvPlayer::serve().await
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
