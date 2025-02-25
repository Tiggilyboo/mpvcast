pub mod mpvcast {
    pub mod control {
        include!(concat!(env!("OUT_DIR"), "/mpvcast.control.rs"));
    }
}

pub use control::{Action, Units};
pub use mpvcast::control::*;

fn create_request(
    action: Action,
    amount: Option<i64>,
    unit: Option<Units>,
    path: Option<String>,
) -> Control {
    Control {
        action: action as i32,
        amount,
        path,
        unit: unit.map(|u| u as i32),
    }
}

pub type Request = Control;

static CONTROL_STOP: Control = Control {
    action: Action::Stop as i32,
    amount: None,
    path: None,
    unit: None,
};

static CONTROL_PAUSE: Control = Control {
    action: Action::Pause as i32,
    amount: None,
    path: None,
    unit: None,
};

static CONTROL_START: Control = Control {
    action: Action::Start as i32,
    amount: None,
    path: None,
    unit: None,
};

impl Request {
    pub fn stop() -> &'static Control {
        &CONTROL_STOP
    }
    pub fn start() -> &'static Control {
        &CONTROL_START
    }
    pub fn pause() -> &'static Control {
        &CONTROL_PAUSE
    }
    pub fn seek(seek: std::time::Duration, units: Units) -> Control {
        create_request(Action::Seek, Some(seek.as_secs() as i64), Some(units), None)
    }
    pub fn volume(volume: i64) -> Control {
        create_request(Action::Volume, Some(volume as i64), None, None)
    }
    pub fn load(path: String) -> Control {
        create_request(Action::Load, None, None, Some(path))
    }
}
