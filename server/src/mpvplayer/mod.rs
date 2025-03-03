use async_std::channel::Sender;
use async_std::io::WriteExt;
use async_std::net::{TcpListener, TcpStream};
use async_std::prelude::*;
use async_std::sync::Mutex;
use comms::Message;
use std::collections::HashSet;
use std::path::Path;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{self, Duration},
};
use url::Url;

use anyhow::{anyhow, Result};
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
    response_tx: Sender<comms::Response>,
    supported_fe: HashSet<String>,
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
    pub async fn serve() -> Result<()> {
        let mpv: Arc<Mpv>;
        match Mpv::new() {
            Ok(data) => mpv = Arc::new(data),
            Err(e) => return Err(anyhow!("Unable to create mpv instance: {:?}", e)),
        }
        let state = RwLock::new(MpvPlayerState {
            state: PlayerState::Idle,
            volume: 0,
        });
        let ffmpeg_format_output = std::process::Command::new("ffmpeg")
            .arg("-formats")
            .output()
            .map_err(|e| anyhow!(e.to_string()))?;

        let supported_fe = String::from_utf8_lossy(&ffmpeg_format_output.stdout)
            .lines()
            .filter_map(|l| {
                let start = l.trim_start();
                if start.starts_with("DE") || start.starts_with("D ") {
                    start
                        .split_whitespace()
                        .nth(1)
                        .map(|s| s.to_lowercase().split(',').collect())
                } else {
                    None
                }
            })
            .collect();

        println!("Supported media formats: {:?}", &supported_fe);

        let (request_tx, request_rx) = async_std::channel::unbounded::<comms::Request>();
        let (response_tx, response_rx) = async_std::channel::unbounded::<comms::Response>();
        let mpv_player = Arc::new(Mutex::new(Self {
            mpv,
            state,
            response_tx,
            supported_fe,
        }));
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

                    let stream_clone = stream.clone();
                    async_std::task::spawn(receive_requests(stream_clone, request_tx.clone()));

                    // Start task handler which forwards responses to stream
                    let stream_response_rx = Arc::clone(&response_rx);
                    async_std::task::spawn(send_responses(stream, stream_response_rx));
                }

                println!("Connection lost, restarting server... {}", attempts);
                if attempts > 5 {
                    break;
                }

                attempts += 1;
            }

            Ok(())
        });

        let mpv_player_cloned = mpv_player.clone();
        async_std::task::spawn(async move {
            while let Ok(request) = request_rx.recv().await {
                let locked_player = mpv_player.lock().await;
                let response = locked_player.process_request(&request);
                println!("Processed response {:?} -> {:?}", &request, &response);
                if let Err(e) = locked_player.response_tx.send(response).await {
                    eprintln!("unable to send response in channel: {:?}", e);
                }
            }
        });

        // Wait until the player has exited
        loop {
            let mpv_player = mpv_player_cloned.lock().await;
            match mpv_player.player_state() {
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

    fn process_request(&self, request: &comms::Request) -> comms::Response {
        println!("process_request: {:?}", request);

        match request.action() {
            comms::Action::Browse => self.browse(request),
            comms::Action::Load => self.load_video(request),
            comms::Action::Stop => self.stop(request),
            comms::Action::Pause => self.pause(request),
            comms::Action::Start => self.play(request),
            comms::Action::Volume => self.set_volume(request),
            comms::Action::Seek => {
                let (seek_time, rewind) = match request.unit() {
                    comms::Units::Seconds => (
                        Duration::from_secs(request.amount().abs() as u64),
                        request.amount() < 0,
                    ),
                    comms::Units::None => (Duration::ZERO, false),
                };
                if !seek_time.is_zero() {
                    self.set_seek(request, seek_time, rewind)
                } else {
                    comms::Response::success(request)
                }
            }
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

    fn set_volume(&self, request: &comms::Request) -> comms::Response {
        let volume = request.amount();
        if let Err(e) = self.set_property(MpvProperty::Volume, volume) {
            return comms::Response::error(request, e.to_string());
        }

        match self.state.write() {
            Ok(mut state) => {
                state.volume = volume;
                comms::Response::success(request)
            }
            Err(e) => comms::Response::error(
                request,
                format!("Unable to set state for volume: {:?}", e.to_string()),
            ),
        }
    }

    fn set_seek(
        &self,
        request: &comms::Request,
        seek: time::Duration,
        rewind: bool,
    ) -> comms::Response {
        self.set_player_state(PlayerState::Seeking);
        let amount = if rewind {
            -seek.as_secs_f64()
        } else {
            seek.as_secs_f64()
        };
        match self.mpv.seek_absolute(amount) {
            Ok(_) => comms::Response::success(request),
            Err(e) => comms::Response::error(request, e.to_string()),
        }
    }

    fn play(&self, request: &comms::Request) -> comms::Response {
        match self.mpv.unpause() {
            Ok(_) => comms::Response::success(request),
            Err(e) => comms::Response::error(request, e.to_string()),
        }
    }

    fn pause(&self, request: &comms::Request) -> comms::Response {
        match self.mpv.pause() {
            Ok(_) => comms::Response::success(request),
            Err(e) => comms::Response::error(request, e.to_string()),
        }
    }

    fn stop(&self, request: &comms::Request) -> comms::Response {
        match self.mpv.playlist_remove_current() {
            Ok(_) => comms::Response::success(request),
            Err(e) => comms::Response::error(request, e.to_string()),
        }
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

    fn browse(&self, request: &comms::Request) -> comms::Response {
        let url_or_path = request.path();
        let mut is_path = false;

        if let Ok(url) = Url::parse(url_or_path) {
            if url.scheme() == "file" {
                is_path = true;
            } else {
                // TODO: browsing URL with yt-dlp?
                return comms::Response::success(request);
            }
        } else {
            is_path = true;
        }

        if is_path {
            let path = Path::new(url_or_path);
            if !path.exists() {
                return comms::Response::error(request, "URL or path does not exist".to_string());
            }
            if path.is_file() {
                return comms::Response::success(request)
                    .with_payload(vec![String::from(url_or_path)]);
            }
            if path.is_dir() {
                let mut playable_paths = Vec::new();
                for entry in walkdir::WalkDir::new(path)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    let path = entry.path();
                    if let Some(path_ext) = path.extension() {
                        let path_ext_str = path_ext.to_str().to_owned().unwrap();
                        if self.supported_fe.contains(path_ext_str) {
                            playable_paths.push(path_ext_str.to_string());
                        }
                    }
                }

                return comms::Response::success(request).with_payload(playable_paths);
            }
        }

        return comms::Response::error(request, "Unhandled path or url".to_string());
    }

    fn load_video(&self, request: &comms::Request) -> comms::Response {
        let url_or_path = request.path();

        if let Err(e) = self.mpv.playlist_clear() {
            return comms::Response::error(
                request,
                format!("Unable to clear playlist: {:?}", e.to_string()),
            );
        }
        if self
            .mpv
            .playlist_load_files(&[(&url_or_path, FileState::Replace, None)])
            .is_err()
        {
            return comms::Response::error(
                request,
                format!("Unable to load files from {}", url_or_path),
            );
        }

        let mut ev_ctx = self.mpv.create_event_context();
        if let Err(e) = ev_ctx.disable_deprecated_events() {
            return comms::Response::error(
                request,
                format!("Unable to disable deprecated events: {:?}", e),
            );
        }
        if let Err(e) = ev_ctx.observe_property(
            MpvProperty::Volume.name(),
            Format::Int64,
            MpvProperty::Volume as u64,
        ) {
            return comms::Response::error(
                request,
                format!("Unable to bind volume in mpv event context: {:?}", e),
            );
        }
        if let Err(e) = ev_ctx.observe_property(
            MpvProperty::DemuxerCacheState.name(),
            Format::Node,
            MpvProperty::DemuxerCacheState as u64,
        ) {
            return comms::Response::error(
                request,
                format!(
                    "Unable to bind demuxer cache state in mpv event context: {:?}",
                    e
                ),
            );
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

        comms::Response::success(request)
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

async fn send_responses(
    mut stream: TcpStream,
    response_rx: Arc<Mutex<async_std::channel::Receiver<comms::Response>>>,
) {
    let response_rx = response_rx.lock().await;
    loop {
        match response_rx.recv().await {
            Ok(response) => {
                let encoded_data = response.encode_length_delimited_to_vec();

                println!(
                    "Writing {} bytes to the stream: {:?}",
                    encoded_data.len(),
                    &encoded_data
                );
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
    mut stream: TcpStream,
    request_tx: async_std::channel::Sender<comms::Request>,
) {
    println!("New client connecting");

    let mut buf = vec![0u8; 1024];

    loop {
        // TODO: Peek doesn't work here and allocating 1024 is not nice
        // Ideally we peek length delimiter, allocate the rest and use a cursor
        match stream.read(&mut buf).await {
            Ok(0) => break,
            Ok(len) => match comms::Request::decode_length_delimited(&buf[..len]) {
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
