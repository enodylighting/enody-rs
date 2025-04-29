pub type Identifier = uuid::Uuid;
pub type Configuration = ();

pub mod interface;
pub mod message;
pub mod remote;

#[derive(Debug)]
pub enum Error {
    Unknown,
    Debug(&'static str),
    Unsupported,
    USB(rusb::Error),
    Serialization,
    Busy,
    InsufficientData
}   