use anyhow::{anyhow, Result};
use async_std::channel::{Receiver, Sender};
use async_std::io::{ReadExt, WriteExt};
use async_std::net::TcpStream;
use clap::Parser;
use comms::Message;
use gtk::glib::clone;
use gtk::prelude::{BoxExt, ButtonExt, GtkWindowExt, *};
use gtk4::{self as gtk, DialogFlags};
use relm4::component::{AsyncComponent, AsyncComponentParts, AsyncComponentSender};
use relm4::{RelmApp, RelmWidgetExt};
use std::fmt::Debug;
use std::time::Duration;

#[derive(Debug)]
enum UiMessage {
    Play,
    Pause,
    Stop,
    Load(String),
    SetVolume(f64),
    SeekAbsolute(f64),
}

#[derive(Debug, Clone, Copy)]
enum UiState {
    Unknown,
    Idle,
    Playing,
    Paused,
    Loading,
    Seeking,
}

struct App {
    mpris: Option<mpris_server::Player>,
    state: UiState,
    request_tx: async_std::channel::Sender<comms::Request>,
    request_id: std::sync::atomic::AtomicU32,
    response_rx: async_std::channel::Receiver<comms::Response>,
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

    pub fn set_status(&self, status: UiState) {
        self.status_label.set_label(&format!("{:#?}", status));
    }
}

impl AsyncComponent for App {
    type Init = Args;
    type Input = UiMessage;
    type Output = ();
    type CommandOutput = ();
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
        load_button.connect_clicked(clone!(
            #[strong]
            sender,
            move |_| {
                sender.input(UiMessage::Load(
                    args.source.clone().unwrap_or("?".to_string()),
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
        if model.mpris.is_none() {
            let emsg = gtk::MessageDialog::new(
                Some(&window),
                DialogFlags::empty(),
                gtk4::MessageType::Error,
                gtk4::ButtonsType::Ok,
                "Unable to initialize MPRIS Server. Check that you are running something with dbus",
            );
            emsg.show();
        }

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
    }

    fn update_view(&self, widgets: &mut Self::Widgets, _sender: AsyncComponentSender<Self>) {
        widgets.set_status(self.state);
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
            .await;

        let mpris = match mpris {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Error initializing MPRIS: {:?}", e);
                None
            }
        };
        let (request_tx, request_rx) = async_std::channel::unbounded::<comms::Request>();
        let (response_tx, response_rx) = async_std::channel::unbounded::<comms::Response>();

        println!("Spawning connection task");
        async_std::task::spawn(start_comms("0.0.0.0:8008", request_rx, response_tx));

        Self {
            mpris,
            request_tx,
            response_rx,
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
}

async fn send_requests(mut con: TcpStream, request_rx: Receiver<comms::Request>) {
    let mut buf = vec![0u8; 1024];
    loop {
        // Try reading any new requests to send from our UI
        match request_rx.try_recv() {
            Ok(request) => {
                if let Err(e) = request.encode(&mut buf) {
                    eprintln!("Error encoding request to bytes: {:?}", e);
                    break;
                }
                if let Err(e) = con.write_all(&buf).await {
                    eprintln!("Error writing request to tcp: {:?}", e);
                    break;
                }
            }
            Err(async_std::channel::TryRecvError::Empty) => (),
            Err(e) => {
                eprintln!("Error receiving request: {:?}", e);
                break;
            }
        }
    }
}

async fn receive_responses(mut con: TcpStream, response_tx: Sender<comms::Response>) {
    let mut reader = async_std::io::BufReader::new(&con);
    let mut buf = [0u8; 1024];

    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        match comms::Response::decode(&buf[..n]) {
            Ok(response) => {
                let id = response.request_id;
                let status = response.status().as_str_name();
                if response_tx.send(response).await.is_err() {
                    eprintln!(
                        "Failed to send response to channel, dropped: id = {}, status = {}",
                        id, status
                    );
                    break;
                }
            }
            Err(e) => {
                eprint!("Failed to decode response bytes: {:?}", e);
                continue;
            }
        }
    }
}

async fn start_comms(
    addr: &str,
    request_rx: Receiver<comms::Request>,
    response_tx: Sender<comms::Response>,
) {
    loop {
        let con = match connect_with_retries(addr).await {
            Some(con) => con,
            None => continue,
        };

        let con_clone = con.clone();
        let send_task = async_std::task::spawn(send_requests(con, request_rx.clone()));
        let recv_task = async_std::task::spawn(receive_responses(con_clone, response_tx.clone()));

        // Wait for either to fail
        send_task.await;
        recv_task.await;

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
