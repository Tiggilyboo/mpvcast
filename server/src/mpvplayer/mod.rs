use async_std::channel::{Receiver, Sender};
use async_std::io::BufReader;
use async_std::net::{TcpListener, TcpStream};
use async_std::prelude::*;
use async_std::sync::Mutex;
use comms::Message;

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{self, Duration},
};

use anyhow::{anyhow, bail, Result};
use events::*;
use libmpv::*;

const TCP_PORT: u16 = 8008;

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
    state: RwLock<MpvPlayerState>,
    request_rx: Receiver<comms::Request>,
    response_tx: Sender<comms::Response>,
}

#[derive(Debug)]
pub struct MpvPlayerState {
    state: PlayerState,
    volume: i64,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum PlayerState {
    Unknown,
    Idle,
    Loaded,
    Playing,
    Paused,
    Seeking,
    Exiting,
}

impl MpvPlayer {
    pub async fn new() -> Result<Self> {
        let mpv: Arc<Mpv>;
        match Mpv::new() {
            Ok(data) => mpv = Arc::new(data),
            Err(e) => return Err(anyhow!("Unable to create mpv instance: {:?}", e)),
        }
        let state = RwLock::new(MpvPlayerState {
            state: PlayerState::Idle,
            volume: 0,
        });
        let (request_tx, request_rx) = async_std::channel::unbounded::<comms::Request>();
        let (response_tx, response_rx) = async_std::channel::unbounded::<comms::Response>();
        let mpv_player = Self {
            mpv,
            state,
            request_rx,
            response_tx,
        };
        let response_rx = Arc::new(Mutex::new(response_rx));

        let connection_string = format!("0.0.0.0:{}", TCP_PORT);
        println!("Starting server at {}", connection_string);
        async_std::task::spawn(async move {
            let mut attempts = 0;
            loop {
                let listener = TcpListener::bind(&connection_string).await;
                if let Err(e) = listener {
                    return Err(anyhow!(e.to_string()));
                }
                let listener = listener.unwrap();

                while let Ok((stream, _)) = listener.accept().await {
                    // Success, restart attempt count
                    attempts = 0;

                    let stream = Arc::new(Mutex::new(stream));
                    async_std::task::spawn(receive_requests(stream.clone(), request_tx.clone()));

                    // Start task handler which forwards responses to stream
                    let stream_response_rx = Arc::clone(&response_rx);
                    async_std::task::spawn(send_response(stream, stream_response_rx));
                }

                println!("Connection lost, restarting server... {}", attempts);
                if attempts > 5 {
                    break;
                }

                attempts += 1;
            }

            Ok(())
        });

        Ok(mpv_player)
    }

    fn process_request(&self, request: comms::Request) {
        println!("process_request: {:?}", request);

        let result = match request.action() {
            comms::Action::Load => self.load_video(request.path()),
            comms::Action::Stop => self.stop(),
            comms::Action::Pause => self.pause(),
            comms::Action::Start => self.play(),
            comms::Action::Volume => self.set_volume(request.amount()),
            comms::Action::Seek => {
                let (seek_time, rewind) = match request.unit() {
                    comms::Units::Seconds => (
                        Duration::from_secs(request.amount().abs() as u64),
                        request.amount() < 0,
                    ),
                    comms::Units::None => (Duration::ZERO, false),
                };
                if !seek_time.is_zero() {
                    self.set_seek(seek_time, rewind)
                } else {
                    Ok(())
                }
            }
        };

        match result {
            Ok(_) => (),
            Err(e) => println!("Unable to process request: {:?}", e),
        }
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

    fn set_volume(&self, volume: i64) -> Result<()> {
        if let Err(e) = self.set_property(MpvProperty::Volume, volume) {
            return Err(e);
        }
        if let Ok(mut state) = self.state.write() {
            state.volume = volume;
        } else {
            return Err(anyhow!("Unable to set volume: Cannot lock state"));
        }

        Ok(())
    }

    fn set_seek(&self, seek: time::Duration, rewind: bool) -> Result<()> {
        self.set_player_state(PlayerState::Seeking);
        let amount = if rewind {
            -seek.as_secs_f64()
        } else {
            seek.as_secs_f64()
        };
        match self.mpv.seek_absolute(amount) {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow!("Unable to set seek: {:?}", e)),
        }
    }

    fn play(&self) -> Result<()> {
        self.mpv.unpause().map_err(|e| anyhow!(e.to_string()))
    }

    fn pause(&self) -> Result<()> {
        self.mpv.pause().map_err(|e| anyhow!(e.to_string()))
    }

    fn stop(&self) -> Result<()> {
        self.mpv
            .playlist_remove_current()
            .map_err(|e| anyhow!(e.to_string()))
    }

    // Should only be called from event handler
    fn set_player_state(&self, new_state: PlayerState) {
        if let Ok(mut state) = self.state.write() {
            if state.state != new_state {
                println!("set_player_state: {:?} -> {:?}", state.state, new_state);
            }
            state.state = new_state;
        }
    }

    pub fn player_state(&self) -> PlayerState {
        if let Ok(state) = self.state.read() {
            state.state
        } else {
            PlayerState::Unknown
        }
    }

    fn load_video(&self, url_or_path: &str) -> Result<()> {
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
            if let Some(ev) = ev_ctx.wait_event(1000.) {
                match ev {
                    Ok(Event::EndFile(r)) => {
                        println!("Idle, stream ended: {:?}", r);
                        self.set_player_state(PlayerState::Idle);
                        break;
                    }
                    Ok(Event::PropertyChange { name, .. })
                        if { name == MpvProperty::Volume.name() } =>
                    {
                        println!("Volume changed");
                    }
                    Ok(Event::PropertyChange {
                        name,
                        change: PropertyData::Node(mpv_node),
                        ..
                    }) if { name == MpvProperty::DemuxerCacheState.name() } => {
                        let ranges = seekable_ranges(mpv_node);
                        println!("Seekable ranges updated: {:?}", ranges);
                    }
                    Ok(Event::Shutdown) => {
                        self.set_player_state(PlayerState::Exiting);
                        break;
                    }
                    Ok(Event::FileLoaded) => {
                        self.set_player_state(PlayerState::Loaded);
                    }
                    Ok(Event::StartFile) => self.set_player_state(PlayerState::Playing),
                    Ok(Event::Seek) => self.set_player_state(PlayerState::Seeking),
                    Ok(Event::PlaybackRestart) => self.set_player_state(PlayerState::Playing),
                    Ok(e) => println!("Event triggered: {:?}", e),
                    Err(e) => println!("Event errored: {:?}", e),
                }
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

async fn send_response(
    stream: Arc<Mutex<TcpStream>>,
    response_rx: Arc<Mutex<async_std::channel::Receiver<comms::Response>>>,
) {
    let response_rx = response_rx.lock().await;
    loop {
        match response_rx.recv().await {
            Ok(response) => {
                let encoded_data = response.encode_to_vec();
                let mut stream = stream.lock().await;
                if let Err(e) = stream.write_all(&encoded_data).await {
                    eprintln!("Error sending response to client: {}", e);
                    continue;
                }
            }
            Err(e) => {
                eprintln!("Error receiving response to send to client: {}", e);
                break;
            }
        }
    }

    println!("Response rx channel EOF");
}

async fn receive_requests(
    stream: Arc<Mutex<TcpStream>>,
    request_tx: async_std::channel::Sender<comms::Request>,
) {
    println!("New client connecting");

    let mut buf_reader = BufReader::new(stream.lock().await.clone());
    let mut buf = vec![0u8; 1024];

    loop {
        match buf_reader.read(&mut buf).await {
            Ok(0) => {
                println!("Client disconnected");
                break;
            }
            Ok(n) => match comms::Request::decode(&buf[..n]) {
                Ok(request) => {
                    println!("Received request: {:?}", request);
                    if let Err(e) = request_tx.send(request).await {
                        eprintln!("Unable to send request to handler: {}", e);
                        continue;
                    }
                }
                Err(e) => {
                    eprintln!("Unable to decode request: {:?}", e);
                    continue;
                }
            },
            Err(e) => {
                eprintln!("Error reading from stream: {:?}", e);
                break;
            }
        }
    }

    println!("Client connection EOF");
}
