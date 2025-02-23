mod mpvcast;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time,
};

use anyhow::{anyhow, bail, Result};
use events::*;
use libmpv::*;
use mpvcast::Request;

#[derive(Copy, Clone)]
enum MpvProperty {
    Volume,
    DemuxerCacheState,
}

// https://github.com/mpv-player/mpv/blob/master/DOCS/man/input.rst
impl MpvProperty {
    fn name(self) -> &'static str {
        match self {
            MpvProperty::Volume => "volume",
            MpvProperty::DemuxerCacheState => "demuxer-cache-state",
        }
    }
}

pub struct MpvPlayer {
    mpv: Arc<Mpv>,
    state: Mutex<MpvPlayerState>,
    requests: Arc<Mutex<VecDeque<Request>>>,
}

#[derive(Debug)]
pub struct MpvPlayerState {
    state: PlayerState,
    volume: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Loaded,
    Playing,
    Paused,
    Seeking,
}

impl MpvPlayer {
    pub fn new() -> Result<Self> {
        let mpv: Arc<Mpv>;
        match Mpv::new() {
            Ok(data) => mpv = Arc::new(data),
            Err(e) => return Err(anyhow!("Unable to create mpv instance: {:?}", e)),
        }
        let state = Mutex::new(MpvPlayerState {
            state: PlayerState::Idle,
            volume: 0,
        });
        let requests = Arc::new(Mutex::new(VecDeque::new()));

        Ok(Self {
            mpv,
            state,
            requests,
        })
    }

    fn set_property<T>(&self, property: MpvProperty, value: T) -> Result<()>
    where
        T: SetData,
    {
        match self.mpv.set_property(property.name(), value) {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow!(
                "Unable to set property '{}': {:?}",
                property.name(),
                e
            )),
        }
    }

    pub fn set_volume(&self, volume: i64) -> Result<()> {
        if let Err(e) = self.set_property(MpvProperty::Volume, volume) {
            return Err(e);
        }
        if let Ok(mut state) = self.state.lock() {
            state.volume = volume;
        } else {
            return Err(anyhow!("Unable to set volume: Cannot lock state"));
        }

        Ok(())
    }

    pub fn set_seek(&self, seek: time::Duration) -> Result<()> {
        self.set_player_state(PlayerState::Seeking);
        match self.mpv.seek_absolute(seek.as_secs_f64()) {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow!("Unable to set seek: {:?}", e)),
        }
    }

    fn set_player_state(&self, new_state: PlayerState) {
        if let Ok(mut state) = self.state.lock() {
            if state.state != new_state {
                println!("set_player_state: {:?} -> {:?}", state.state, new_state);
            }
            state.state = new_state;
        }
    }

    fn queue_request(&self, request: Request) -> bool {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push_back(request);
            true
        } else {
            false
        }
    }

    pub fn stream_video(&self, url_or_path: &str, volume: i64, seek: time::Duration) -> Result<()> {
        if self.mpv.playlist_clear().is_err() {
            bail!("Unable to clear playlist")
        }
        if self
            .mpv
            .playlist_load_files(&[(&url_or_path, FileState::Replace, None)])
            .is_err()
        {
            bail!("Unable to load files from {}", url_or_path)
        }
        if self.set_volume(volume).is_err() {
            bail!("Unable to set volume to {}", volume)
        }

        let mut ev_ctx = self.mpv.create_event_context();
        if ev_ctx.disable_deprecated_events().is_err() {
            bail!("Unable to disable deprecated events")
        }
        if ev_ctx
            .observe_property(
                MpvProperty::Volume.name(),
                Format::Int64,
                MpvProperty::Volume as u64,
            )
            .is_err()
        {
            bail!("Unable to bind volume in mpv event context")
        }
        if ev_ctx
            .observe_property(
                MpvProperty::DemuxerCacheState.name(),
                Format::Node,
                MpvProperty::DemuxerCacheState as u64,
            )
            .is_err()
        {
            bail!("Unable to bind demuxer cache state in mpv event context")
        }

        println!("Starting event loop...");
        loop {
            let ev = ev_ctx.wait_event(1000.).unwrap_or(Err(Error::Null));

            match ev {
                Ok(Event::EndFile(r)) => {
                    println!("Exiting, stream ended: {:?}", r);
                    break;
                }
                Ok(Event::PropertyChange { name, .. })
                    if { name == MpvProperty::Volume.name() } => {}
                Ok(Event::PropertyChange {
                    name,
                    change: PropertyData::Node(mpv_node),
                    ..
                }) if { name == MpvProperty::DemuxerCacheState.name() } => {
                    let ranges = seekable_ranges(mpv_node);
                    println!("Seekable ranges updated: {:?}", ranges);
                }
                Ok(Event::Shutdown) => {
                    println!("Exiting, shutdown");
                    break;
                }
                Ok(Event::FileLoaded) => {
                    // Loaded, seek to the position
                    self.set_seek(seek)?;
                    self.set_player_state(PlayerState::Loaded);
                }
                Ok(Event::StartFile) => self.set_player_state(PlayerState::Playing),
                Ok(Event::Seek) => self.set_player_state(PlayerState::Seeking),
                Ok(e) => println!("Event triggered: {:?}", e),
                Err(e) => println!("Event errored: {:?}", e),
            }
        }
        println!("Event loop exited");

        Ok(())
    }
}

fn seekable_ranges(demuxer_cache_state: &MpvNode) -> Option<Vec<(f64, f64)>> {
    let mut res = Vec::new();
    let props: HashMap<&str, MpvNode> = demuxer_cache_state.to_map()?.collect();
    let ranges = props.get("seekable-ranges")?.to_array()?;

    for node in ranges {
        let range: HashMap<&str, MpvNode> = node.to_map()?.collect();
        let start = range.get("start")?.to_f64()?;
        let end = range.get("end")?.to_f64()?;
        res.push((start, end));
    }

    Some(res)
}
