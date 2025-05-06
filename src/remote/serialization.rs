pub const CONTROL_CHAR_STX: u8 = 0x02;
pub const CONTROL_CHAR_ETX: u8 = 0x03;
pub const CONTROL_CHAR_DLE: u8 = 0x10;

fn escaped_bytes(payload: &[u8]) -> Vec<u8> {
    let mut escaped_bytes = Vec::with_capacity(payload.len() * 2);
    for byte in payload {
        if *byte == CONTROL_CHAR_STX || *byte == CONTROL_CHAR_ETX || *byte == CONTROL_CHAR_DLE {
            escaped_bytes.push(CONTROL_CHAR_DLE);
        }
        escaped_bytes.push(*byte);
    }
    escaped_bytes
}

fn frame_bytes(payload: &[u8]) -> Vec<u8> {
    let mut frame_bytes = Vec::with_capacity((payload.len() * 2) + 2);
    frame_bytes.push(CONTROL_CHAR_STX);
    frame_bytes.extend(escaped_bytes(payload));
    frame_bytes.push(CONTROL_CHAR_ETX);
    frame_bytes
}

fn unescape_bytes(escaped: &[u8]) -> Vec<u8> {
    let mut unescaped: Vec<u8> = Vec::new();
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

fn unframe_bytes(frame: &[u8]) -> Result<Vec<u8>, crate::Error> {
    // Check minimum frame size (STX + ETX)
    if frame.len() < 2 {
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

impl TryFrom<Vec<u8>> for crate::message::Message {
    type Error = crate::Error;
    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let unframed_bytes = unframe_bytes(&bytes)?;
        postcard::from_bytes(&unframed_bytes)
            .map_err(|e| {
                crate::Error::Serialization
            })
    }
}

impl TryFrom<crate::message::Message> for Vec<u8> {
    type Error = crate::Error;
    fn try_from(message: crate::message::Message) -> Result<Self, Self::Error> {
        let message_bytes = postcard::to_allocvec(&message)
            .map_err(|_| crate::Error::Serialization)?;
        let framed_bytes = frame_bytes(&message_bytes);
        Ok(framed_bytes)
    }
}