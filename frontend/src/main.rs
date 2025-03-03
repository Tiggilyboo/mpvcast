use anyhow::{anyhow, Result};
use async_std::channel::{Receiver, Sender};
use async_std::io::{ReadExt, WriteExt};
use async_std::net::TcpStream;
use async_std::sync::RwLock;
use clap::Parser;
use comms::Message;
use gtk::glib::clone;
use gtk::prelude::{BoxExt, ButtonExt, GtkWindowExt, *};
use gtk4::{self as gtk};
use relm4::component::{AsyncComponent, AsyncComponentParts, AsyncComponentSender};
use relm4::{RelmApp, RelmWidgetExt};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
enum UiMessage {
    Play,
    Pause,
    Stop,
    Load(String),
    Browse(String),
    SetVolume(f64),
    SeekAbsolute(f64),
}

#[derive(Debug)]
enum UiCommand {
    UpdateWidgets,
    SendInitialRequests(Args),
}

#[derive(Debug, Clone)]
enum UiState {
    Unknown,
    Error(String),
    Idle,
    Playing,
    Paused,
    Loading,
}

impl UiState {
    pub fn playback_status(&self) -> mpris_server::PlaybackStatus {
        match self {
            UiState::Playing => mpris_server::PlaybackStatus::Playing,
            UiState::Paused => mpris_server::PlaybackStatus::Paused,
            _ => mpris_server::PlaybackStatus::Stopped,
        }
    }

    pub fn can_play(&self) -> bool {
        match self {
            UiState::Paused | UiState::Idle => true,
            UiState::Playing | UiState::Error(_) | UiState::Loading | UiState::Unknown => false,
        }
    }

    pub fn can_pause(&self) -> bool {
        match self {
            UiState::Playing => true,
            _ => false,
        }
    }

    pub fn can_stop(&self) -> bool {
        match self {
            UiState::Playing | UiState::Paused => true,
            _ => false,
        }
    }

    pub fn can_seek(&self) -> bool {
        match self {
            UiState::Playing | UiState::Paused | UiState::Loading => true,
            _ => false,
        }
    }
}

struct MediaState {
    title: String,
    length: std::time::Duration,
    current: std::time::Duration,
}

struct App {
    mpris: mpris_server::Player,
    state: UiState,
    media: Option<MediaState>,
    request_tx: async_std::channel::Sender<comms::Request>,
    request_id: std::sync::atomic::AtomicU32,
    response_rx: async_std::channel::Receiver<(comms::Request, comms::Response)>,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    source: Option<String>,

    #[arg(short, long, value_parser = clap::value_parser!(u8).range(0..100))]
    volume: Option<usize>,

    #[arg(short, long)]
    #[arg(value_parser = parse_duration)]
    at: Option<Duration>,
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

struct AppWidgets {
    window: gtk::Window,
    play_button: gtk::Button,
    pause_button: gtk::Button,
    stop_button: gtk::Button,
    load_button: gtk::Button,
    mute_button: gtk::Button,
    volume_slider: gtk::Scale,
    seek_slider: gtk::Scale,
    time_label: gtk::Label,
    end_label: gtk::Label,
    status_label: gtk::Label,
}

impl AppWidgets {
    pub fn disable(&mut self) {
        self.window.set_sensitive(false);
    }
    pub fn enable(&mut self) {
        self.window.set_sensitive(true);
    }

    pub fn set_status(&self, state: &UiState) {
        let status = match state {
            UiState::Error(err) => err,
            _ => &format!("{:#?}", state),
        };

        self.status_label.set_label(status);
    }

    pub fn set_media_state(&self, media: Option<&MediaState>) {
        if let Some(media) = media {
            self.time_label.set_label(&format!("{:?}", media.current));
            self.end_label.set_label(&format!("{:?}", media.length));
            self.window
                .set_title(Some(&format!("Casting {}", media.title)));
        } else {
            self.time_label.set_label("00:00:00");
            self.end_label.set_label("00:00:00");
            self.window.set_title(Some("MpvCast"));
        }
    }
}

impl AsyncComponent for App {
    type Init = Args;
    type Input = UiMessage;
    type Output = ();
    type CommandOutput = UiCommand;
    type Widgets = AppWidgets;
    type Root = gtk::Window;

    fn init_root() -> Self::Root {
        gtk::Window::builder().title("MpvCaster.Frontend").build()
    }

    async fn init(
        args: Self::Init,
        window: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        println!("Init: started");
        let v_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(5)
            .build();

        let control_box = gtk::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(5)
            .build();

        v_box.append(&control_box);
        window.set_child(Some(&v_box));

        println!("Init: buttons");
        let play_button = gtk::Button::from_icon_name("media-playback-start-symbolic");
        let pause_button = gtk::Button::from_icon_name("media-player-puase-symbolic");
        let stop_button = gtk::Button::from_icon_name("media-playback-stop-symbolic");
        let load_button = gtk::Button::from_icon_name("edit-paste-symbolic");
        let mute_button = gtk::Button::from_icon_name("audio-volume-muted-symbolic");

        println!("Init: sliders");
        let volume_slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1.0, 0.1);
        let seek_slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1.0, 0.1);

        println!("Init: labels");
        let time_label = gtk::Label::new(Some("00:00:00"));
        let end_label = gtk::Label::new(Some("00:00:00"));
        time_label.set_margin_all(5);
        end_label.set_margin_all(5);

        let status_label = gtk::Label::new(Some("Initializing..."));
        status_label.set_margin_all(5);
        v_box.append(&status_label);

        println!("Init: window & control box");
        control_box.append(&mute_button);
        control_box.append(&volume_slider);
        control_box.append(&load_button);
        control_box.append(&pause_button);
        control_box.append(&play_button);
        control_box.append(&stop_button);
        control_box.append(&time_label);
        control_box.append(&seek_slider);
        control_box.append(&end_label);

        println!("Init: connecting events");
        play_button.connect_clicked(clone!(
            #[strong]
            sender,
            move |_| {
                sender.input(UiMessage::Play);
            }
        ));
        pause_button.connect_clicked(clone!(
            #[strong]
            sender,
            move |_| {
                sender.input(UiMessage::Pause);
            }
        ));
        stop_button.connect_clicked(clone!(
            #[strong]
            sender,
            move |_| {
                sender.input(UiMessage::Stop);
            }
        ));
        mute_button.connect_clicked(clone!(
            #[strong]
            sender,
            move |_| {
                sender.input(UiMessage::SetVolume(0.));
            }
        ));
        let source_cloned = args.source.clone();
        load_button.connect_clicked(clone!(
            #[strong]
            sender,
            move |_| {
                sender.input(UiMessage::Load(
                    source_cloned.clone().unwrap_or("?".to_string()),
                ));
            }
        ));
        volume_slider.connect_change_value(clone!(
            #[strong]
            sender,
            move |_, _st: gtk::ScrollType, value: f64| {
                sender.input(UiMessage::SetVolume(value));
                gtk4::glib::Propagation::Proceed
            }
        ));
        seek_slider.connect_change_value(clone!(
            #[strong]
            sender,
            move |_, _st: gtk::ScrollType, value: f64| {
                sender.input(UiMessage::SeekAbsolute(value));
                gtk4::glib::Propagation::Proceed
            }
        ));

        println!("Init: creating app");
        let model = App::new().await;

        let widgets = AppWidgets {
            window,
            mute_button,
            load_button,
            play_button,
            pause_button,
            stop_button,
            volume_slider,
            seek_slider,
            time_label,
            end_label,
            status_label,
        };
        sender.command(|out, shutdown| {
            shutdown
                .register(async move { out.send(UiCommand::UpdateWidgets).unwrap() })
                .drop_on_shutdown()
        });
        sender.command(|out, shutdown| {
            shutdown
                .register(async move { out.send(UiCommand::SendInitialRequests(args)).unwrap() })
                .drop_on_shutdown()
        });

        println!("Initialized view");
        AsyncComponentParts { model, widgets }
    }

    async fn update(
        &mut self,
        message: Self::Input,
        _sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        let request = match message {
            UiMessage::Play => comms::Request::start(self.next_request_id()),
            UiMessage::Pause => comms::Request::pause(self.next_request_id()),
            UiMessage::Stop => comms::Request::stop(self.next_request_id()),
            UiMessage::Load(path) => comms::Request::load(self.next_request_id(), path),
            UiMessage::Browse(path) => comms::Request::browse(self.next_request_id(), path),
            UiMessage::SetVolume(volume) => {
                comms::Request::volume(self.next_request_id(), (volume * 100.) as i64)
            }
            UiMessage::SeekAbsolute(time) => comms::Request::seek(
                self.next_request_id(),
                Duration::from_secs(time as u64),
                comms::Units::None,
            ),
        };

        if let Err(e) = self.queue_request(request).await {
            eprintln!("{}", e);
        }

        // Wait for response to be processed
        let pair = match self.response_rx.recv().await {
            Ok(pair) => Some(pair),
            Err(e) => {
                self.state = UiState::Error(e.to_string());
                None
            }
        };
        if let Some(pair) = pair {
            self.handle_comms_pair(pair).await;
        }
    }

    async fn update_cmd_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        message: Self::CommandOutput,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) -> () {
        let state = &self.state;
        widgets.set_status(state);
        widgets.set_media_state(self.media.as_ref());

        match state {
            UiState::Loading | UiState::Unknown | UiState::Error(_) => widgets.disable(),
            _ => widgets.enable(),
        }

        match message {
            UiCommand::UpdateWidgets => {
                println!("UpdateWidgets");

                widgets.play_button.set_visible(false);
                widgets.pause_button.set_visible(false);
                widgets.stop_button.set_visible(false);
                widgets.mute_button.set_visible(false);
                widgets.volume_slider.set_visible(false);
                widgets.seek_slider.set_visible(false);
                widgets.load_button.set_visible(false);

                // mpris
                let mpris = &self.mpris;
                if let Err(e) = mpris.set_playback_status(state.playback_status()).await {
                    eprintln!("{}", e);
                }

                let can_play = state.can_play();
                widgets.play_button.set_visible(can_play);
                if let Err(e) = mpris.set_can_play(can_play).await {
                    eprintln!("{}", e);
                }

                let can_pause = state.can_pause();
                widgets.pause_button.set_visible(can_pause);
                if let Err(e) = mpris.set_can_pause(can_pause).await {
                    eprintln!("{}", e);
                }

                let can_stop = state.can_stop();
                widgets.stop_button.set_visible(can_stop);
                if let Err(e) = mpris.set_can_quit(can_stop).await {
                    eprintln!("{}", e);
                }

                let can_seek = state.can_seek();
                widgets.seek_slider.set_visible(can_seek);
                if let Err(e) = mpris.set_can_seek(can_seek).await {
                    eprintln!("{}", e);
                }
            }
            UiCommand::SendInitialRequests(args) => {
                println!("SendInitialRequestArgs: {:?}", &args);

                if let Some(initial_source) = args.source {
                    let initial_load = UiMessage::Load(initial_source);
                    if let Err(e) = sender.input_sender().send(initial_load) {
                        eprintln!("Error sending initial source request: {:?}", e);
                    }
                } else {
                    let initial_browse = UiMessage::Browse("~/Videos".to_string());
                    if let Err(e) = sender.input_sender().send(initial_browse) {
                        eprintln!("error sending initial browse request: {:?}", e);
                    }
                }
                if let Some(initial_volume) = args.volume {
                    println!("Sending initial source: {:?}", &initial_volume);
                    sender
                        .input_sender()
                        .emit(UiMessage::SetVolume(initial_volume as f64 / 100.));
                }
                if let Some(initial_seek) = args.at {
                    println!("Sending initial seek: {:?}", &initial_seek);
                    sender
                        .input_sender()
                        .emit(UiMessage::SeekAbsolute(initial_seek.as_secs_f64()));
                }
            }
        }
    }
}

impl App {
    pub async fn new() -> Self {
        println!("Initializing MPRIS server");
        let mpris = mpris_server::Player::builder("mpvcast.frontend")
            .can_pause(false)
            .can_play(false)
            .can_quit(false)
            .can_seek(false)
            .can_go_next(false)
            .can_go_previous(false)
            .can_set_fullscreen(true)
            .can_control(true)
            .build()
            .await
            .unwrap();

        let (request_tx, request_rx) = async_std::channel::unbounded::<comms::Request>();
        let (response_tx, response_rx) =
            async_std::channel::unbounded::<(comms::Request, comms::Response)>();

        println!("Spawning connection task");
        async_std::task::spawn(start_comms(
            "0.0.0.0:8008",
            Arc::new(request_rx),
            response_tx,
        ));

        async_std::task::spawn_local(mpris.run());

        Self {
            mpris,
            request_tx,
            response_rx,
            media: None,
            state: UiState::Unknown,
            request_id: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn next_request_id(&self) -> u32 {
        self.request_id
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
    }

    async fn queue_request(&self, request: comms::Request) -> Result<()> {
        println!("queueing request: {:?}", request);

        match self.request_tx.send(request).await {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow!(e.to_string())),
        }
    }

    async fn handle_comms_pair(&mut self, pair: (comms::Request, comms::Response)) {
        println!("handle_comms_pair: {:#?}", pair);
    }
}

async fn receive_responses(
    mut con: TcpStream,
    pending_requests: Arc<RwLock<HashMap<u32, comms::Request>>>,
    response_tx: Sender<(comms::Request, comms::Response)>,
) {
    let mut len_buf = [0u8; 10];

    loop {
        println!("Waiting on responses...");
        if let Ok(n) = con.peek(&mut len_buf).await {
            if n == 0 {
                println!("No data, exiting receive loop");
                break;
            }
        } else {
            eprintln!("Unable to read bytes in stream");
            break;
        }
        if let Ok(len) = comms::decode_length_delimiter(&len_buf[..]) {
            let mut buf = vec![0u8; len];
            if let Err(e) = con.read_exact(&mut buf).await {
                eprintln!("Error reading {} bytes from stream: {}", len, e);
                break;
            }
            match comms::Response::decode(&buf[..]) {
                Ok(response) => {
                    let id = response.request_id;
                    let status = response.status().as_str_name();
                    println!(
                        "Received response from server: id = {}, status = {}",
                        id, status
                    );

                    let mut pending = pending_requests.write().await;
                    if let Some(request) = pending.remove(&id) {
                        println!("Response matched request: {:?}", &request);

                        if response_tx.send((request, response)).await.is_err() {
                            eprintln!(
                                "Failed to send response to channel, dropped: id = {}, status = {}",
                                id, status
                            );
                            break;
                        }
                    } else {
                        eprintln!("Response did not match pending request: {:?}", &response);
                    }
                }
                Err(e) => {
                    eprint!("Failed to decode response bytes: {:?}", e);
                    continue;
                }
            }
        } else {
            eprintln!("Failed to decode length delimiter in request stream");
        }
    }
}

async fn send_requests(
    mut con: TcpStream,
    pending_requests: Arc<RwLock<HashMap<u32, comms::Request>>>,
    request_rx: Arc<async_std::channel::Receiver<comms::Request>>,
) {
    while let Ok(request) = request_rx.recv().await {
        let buf = request.encode_length_delimited_to_vec();
        if let Err(e) = con.write_all(&buf).await {
            eprintln!("Error writing request to tcp: {:?}", e);
            break;
        }
        println!("Wrote request of {} bytes to stream: {:?}", buf.len(), &buf);

        let mut pending = pending_requests.write().await;
        pending.insert(request.id, request);
    }
}

async fn start_comms(
    addr: &str,
    request_rx: Arc<Receiver<comms::Request>>,
    response_tx: Sender<(comms::Request, comms::Response)>,
) {
    let pending_requests = Arc::new(RwLock::new(HashMap::<u32, comms::Request>::new()));
    loop {
        let con = match connect_with_retries(addr).await {
            Some(con) => con,
            None => continue,
        };

        let send_task = async_std::task::spawn(send_requests(
            con.clone(),
            pending_requests.clone(),
            request_rx.clone(),
        ));
        let recv_task = async_std::task::spawn(receive_responses(
            con.clone(),
            pending_requests.clone(),
            response_tx.clone(),
        ));

        // Wait for either to fail
        recv_task.await;
        send_task.await;

        println!("Connection lost. Reconnecting...");
    }
}

async fn connect_with_retries(addr: &str) -> Option<TcpStream> {
    let mut attempts = 0;

    loop {
        match TcpStream::connect(addr).await {
            Ok(stream) => {
                println!("Connected to {}", addr);
                return Some(stream);
            }
            Err(e) => {
                attempts += 1;
                let delay = Duration::from_secs(2u64.pow(attempts.min(5))); // Max 32s backoff
                println!(
                    "Failed to connect to {} (attempt {}): {}. Retrying in {:?}...",
                    addr, attempts, e, delay
                );
                async_std::task::sleep(delay).await;
            }
        }
    }
}

fn main() {
    let args = Args::parse();
    let app = RelmApp::new("mpvcast.frontend");
    app.run_async::<App>(args);
}
