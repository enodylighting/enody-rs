
#[cfg(feature = "std")]
pub type USBDataBuffer = Vec<u8>;
#[cfg(not(feature = "std"))]
pub type USBDataBuffer = heapless::Vec<u8, 512>;

pub const CONTROL_CHAR_STX: u8 = 0x02;
pub const CONTROL_CHAR_ETX: u8 = 0x03;
pub const CONTROL_CHAR_DLE: u8 = 0x10;

pub const FRAME_SIZE_MIN: usize = 2; // (STX + ETX)

fn escaped_bytes(payload: &[u8]) -> USBDataBuffer {
    let mut escaped_bytes = USBDataBuffer::new();
    for byte in payload {
        if *byte == CONTROL_CHAR_STX || *byte == CONTROL_CHAR_ETX || *byte == CONTROL_CHAR_DLE {
            escaped_bytes.push(CONTROL_CHAR_DLE);
        }
        escaped_bytes.push(*byte);
    }
    escaped_bytes
}

fn unescape_bytes(escaped: &[u8]) -> USBDataBuffer {
    let mut unescaped: USBDataBuffer = USBDataBuffer::new();
    let mut is_escaped = false;

    for &byte in escaped {
        if is_escaped {
            unescaped.push(byte);
            is_escaped = false;
        } else if byte == CONTROL_CHAR_DLE {
            is_escaped = true;
        } else {
            unescaped.push(byte);
        }
    }
    unescaped
}

fn frame_bytes(payload: &[u8]) -> USBDataBuffer {
    let mut frame_bytes = USBDataBuffer::new();
    frame_bytes.push(CONTROL_CHAR_STX);
    frame_bytes.extend(escaped_bytes(payload));
    frame_bytes.push(CONTROL_CHAR_ETX);
    frame_bytes
}

fn unframe_bytes(frame: &[u8]) -> Result<USBDataBuffer, crate::Error> {
    // Check minimum frame size
    if frame.len() < FRAME_SIZE_MIN {
        return Err(crate::Error::InsufficientData);
    }

    // Verify STX and ETX
    if frame[0] != CONTROL_CHAR_STX || frame[frame.len() - 1] != CONTROL_CHAR_ETX {
        return Err(crate::Error::InsufficientData);
    }

    // Extract and unescape the payload between STX and ETX
    let payload = &frame[1..frame.len() - 1];
    Ok(unescape_bytes(payload))
}

impl<InternalCommand, InternalEvent> TryFrom<USBDataBuffer> for crate::message::Message<InternalCommand, InternalEvent>
where
    InternalCommand: serde::de::DeserializeOwned + serde::Serialize,
    InternalEvent: serde::de::DeserializeOwned + serde::Serialize
{
    type Error = crate::Error;
    fn try_from(bytes: USBDataBuffer) -> Result<Self, Self::Error> {
        let unframed_bytes = unframe_bytes(&bytes)?;
        postcard::from_bytes(&unframed_bytes)
            .map_err(|_| crate::Error::Serialization)
    }
}

impl<InternalCommand, InternalEvent> TryFrom<crate::message::Message<InternalCommand, InternalEvent>> for USBDataBuffer
where
    InternalCommand: serde::de::DeserializeOwned + serde::Serialize,
    InternalEvent: serde::de::DeserializeOwned + serde::Serialize
{
    type Error = crate::Error;
    fn try_from(message: crate::message::Message<InternalCommand, InternalEvent>) -> Result<Self, Self::Error> {
        #[cfg(feature = "std")]
        let message_bytes: USBDataBuffer = postcard::to_allocvec(&message).map_err(|_| crate::Error::Serialization)?;
        #[cfg(not(feature = "std"))]
        let message_bytes = postcard::to_vec::<_, 1024>(&message).map_err(|_| crate::Error::Serialization)?;
        let framed_bytes = frame_bytes(&message_bytes);
        Ok(framed_bytes)
    }
}