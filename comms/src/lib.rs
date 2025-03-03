pub mod comms {
    pub mod control {
        include!(concat!(env!("OUT_DIR"), "/mpvcast.comms.rs"));
    }
}

pub use comms::control::control::*;
pub use comms::control::response::*;
use comms::control::Control;
pub use prost::bytes::buf::*;
pub use prost::decode_length_delimiter;
pub use prost::encode_length_delimiter;
pub use prost::Message;

pub type Response = comms::control::Response;
pub type Request = comms::control::Control;

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
    pub fn browse(id: u32, path: String) -> Control {
        create_request(id, Action::Browse, None, None, Some(path))
    }
}

impl From<&Request> for Response {
    fn from(value: &Request) -> Self {
        Self {
            request_id: value.id,
            status: Status::Success.into(),
            payload: Vec::new(),
        }
    }
}

impl Response {
    pub fn success(from_request: &Request) -> Self {
        Response::from(from_request).with_status(Status::Success)
    }

    pub fn error(from_request: &Request, message: String) -> Self {
        Response::from(from_request)
            .with_status(Status::Error)
            .with_message(message)
    }

    pub fn failure(from_request: &Request, message: String) -> Self {
        Response::from(from_request)
            .with_status(Status::Failure)
            .with_message(message)
    }

    pub fn with_status(mut self, status: comms::control::response::Status) -> Self {
        self.set_status(status);
        self
    }

    pub fn with_message(mut self, message: String) -> Self {
        self.payload = vec![message];
        self
    }

    pub fn with_payload(mut self, payload: Vec<String>) -> Self {
        self.payload = payload;
        self
    }
}
