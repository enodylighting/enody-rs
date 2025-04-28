const CONTROL_CHAR_STX: u8 = 0x02;
const CONTROL_CHAR_ETX: u8 = 0x03;
const CONTROL_CHAR_DLE: u8 = 0x10;

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

fn unescape_bytes(escaped: &[u8]) -> heapless::Vec<u8, USB_BUFFER_SIZE> {
    let mut unescaped: heapless::Vec<u8, USB_BUFFER_SIZE> = heapless::Vec::new();
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

fn unframe_bytes(frame: &[u8]) -> Option<heapless::Vec<u8, USB_BUFFER_SIZE>> {
    // Check minimum frame size (STX + ETX)
    if frame.len() < 2 {
        return None;
    }

    // Verify STX and ETX
    if frame[0] != CONTROL_CHAR_STX || frame[frame.len() - 1] != CONTROL_CHAR_ETX {
        return None;
    }

    // Extract and unescape the payload between STX and ETX
    let payload = &frame[1..frame.len() - 1];
    Some(unescape_bytes(payload))
}