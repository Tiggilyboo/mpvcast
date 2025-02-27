pub mod comms {
    pub mod control {
        include!(concat!(env!("OUT_DIR"), "/mpvcast.comms.rs"));
    }
}

pub use comms::control::*;
pub use control::*;
pub use prost::Message;

fn create_request(
    id: u32,
    action: Action,
    amount: Option<i64>,
    unit: Option<Units>,
    path: Option<String>,
) -> Control {
    Control {
        id,
        action: action as i32,
        amount,
        path,
        unit: unit.map(|u| u as i32),
    }
}

pub type Request = Control;

impl Request {
    pub fn stop(id: u32) -> Control {
        create_request(id, Action::Stop, None, None, None)
    }
    pub fn start(id: u32) -> Control {
        create_request(id, Action::Start, None, None, None)
    }
    pub fn pause(id: u32) -> Control {
        create_request(id, Action::Pause, None, None, None)
    }
    pub fn seek(id: u32, seek: std::time::Duration, units: Units) -> Control {
        create_request(
            id,
            Action::Seek,
            Some(seek.as_secs() as i64),
            Some(units),
            None,
        )
    }
    pub fn volume(id: u32, volume: i64) -> Control {
        create_request(id, Action::Volume, Some(volume as i64), None, None)
    }
    pub fn load(id: u32, path: String) -> Control {
        create_request(id, Action::Load, None, None, Some(path))
    }
}
