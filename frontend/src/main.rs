use anyhow::{anyhow, Result};
use async_std::io::WriteExt;
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

const ICON_VOLUME_MUTE: &'static str = "audio-volume-muted-symbolic";
const ICON_VOLUME_LOW: &'static str = "audio-volume-low-symbolic";
const ICON_VOLUME_MEDIUM: &'static str = "audio-volume-medium-symbolic";
const ICON_VOLUME_HIGH: &'static str = "audio-volume-high-symbolic";

#[derive(Debug)]
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
    tx: async_std::channel::Sender<comms::Request>,
    state: UiState,
}

struct AppWidgets {
    window: gtk::Window,
    control_box: gtk::Box,
    play_button: gtk::Button,
    pause_button: gtk::Button,
    load_button: gtk::Button,
    mute_button: gtk::Button,
    volume_slider: gtk::Scale,
    seek_slider: gtk::Scale,
    time_label: gtk::Label,
    end_label: gtk::Label,
}

impl AsyncComponent for App {
    type Init = ();
    type Input = UiMessage;
    type Output = ();
    type CommandOutput = ();
    type Widgets = AppWidgets;
    type Root = gtk::Window;

    fn init_root() -> Self::Root {
        gtk::Window::builder().title("MpvCaster.Frontend").build()
    }

    async fn init(
        _init: Self::Init,
        window: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        println!("Init: started");
        let control_box = gtk::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(5)
            .build();

        println!("Init: buttons");
        let play_button = gtk::Button::from_icon_name("media-playback-start-symbolic");
        println!("Init: buttons");
        let pause_button = gtk::Button::from_icon_name("media-player-puase-symbolic");
        println!("Init: buttons");
        let load_button = gtk::Button::from_icon_name("edit-paste-symbolic");
        println!("Init: buttons");
        let mute_button = gtk::Button::from_icon_name(&ICON_VOLUME_MUTE);
        println!("Init: buttons");

        println!("Init: sliders");
        let volume_slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1.0, 0.1);
        let seek_slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1.0, 0.1);

        println!("Init: labels");
        let time_label = gtk::Label::new(Some("00:00:00"));
        let end_label = gtk::Label::new(Some("00:00:00"));
        time_label.set_margin_all(5);
        end_label.set_margin_all(5);

        println!("Init: window & control box");
        window.set_child(Some(&control_box));
        control_box.append(&mute_button);
        control_box.append(&volume_slider);
        control_box.append(&load_button);
        control_box.append(&pause_button);
        control_box.append(&play_button);
        control_box.append(&seek_slider);
        control_box.append(&time_label);
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
                // TODO
                sender.input(UiMessage::Load(
                    "~/Videos/Shows/Fargo/Fargo.S05E09.mp4".to_string(),
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
            mute_button,
            load_button,
            window,
            control_box,
            play_button,
            pause_button,
            volume_slider,
            seek_slider,
            time_label,
            end_label,
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
            UiMessage::Play => comms::Request::start().clone(),
            UiMessage::Pause => comms::Request::pause().clone(),
            UiMessage::Stop => comms::Request::stop().clone(),
            UiMessage::Load(path) => comms::Request::load(path),
            UiMessage::SetVolume(volume) => comms::Request::volume((volume * 100.) as i64),
            UiMessage::SeekAbsolute(time) => {
                comms::Request::seek(Duration::from_secs(time as u64), comms::Units::None)
            }
        };

        if let Err(e) = self.queue_request(request).await {
            eprintln!("{}", e);
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
            .await;

        let mpris = match mpris {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Error initializing MPRIS: {:?}", e);
                None
            }
        };
        let (tx, rx) = async_std::channel::unbounded::<comms::Request>();

        println!("Spawning request sender task");
        async_std::task::spawn(async move {
            let con = async_std::net::TcpStream::connect("0.0.0.0:8008").await;
            if let Err(e) = con {
                eprintln!("Error connecting to server: {:?}", e);
                return;
            }
            let mut con = con.unwrap();
            let mut buf = vec![0u8; 1024];
            loop {
                let request = rx.recv().await;
                match request {
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
                    Err(e) => {
                        eprintln!("Error receiving request: {:?}", e);
                        break;
                    }
                }
            }

            println!("Request sender task complete");
        });

        Self {
            mpris,
            tx,
            state: UiState::Unknown,
        }
    }

    async fn queue_request(&self, request: comms::Request) -> Result<()> {
        println!("queueing request: {:?}", request);

        match self.tx.send(request).await {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow!(e.to_string())),
        }
    }
}

fn main() {
    let app = RelmApp::new("mpvcast.frontend");
    app.run_async::<App>(());
}
