#[cfg(feature = "std")]
pub type UsbDataBuffer = Vec<u8>;
#[cfg(not(feature = "std"))]
pub type UsbDataBuffer = heapless::Vec<u8, 512>;

macro_rules! buffer_push {
    ($buffer:expr, $byte:expr) => {
        #[cfg(feature = "std")]
        $buffer.push($byte);
        #[cfg(not(feature = "std"))]
        $buffer.push($byte).unwrap();
    };
}

pub const CONTROL_CHAR_STX: u8 = 0x02;
pub const CONTROL_CHAR_ETX: u8 = 0x03;
pub const CONTROL_CHAR_DLE: u8 = 0x10;

pub const FRAME_SIZE_MIN: usize = 2; // (STX + ETX)

fn escaped_bytes(payload: &[u8]) -> UsbDataBuffer {
    let mut escaped_bytes = UsbDataBuffer::new();
    for byte in payload {
        if *byte == CONTROL_CHAR_STX || *byte == CONTROL_CHAR_ETX || *byte == CONTROL_CHAR_DLE {
            buffer_push!(escaped_bytes, CONTROL_CHAR_DLE);
        }
        buffer_push!(escaped_bytes, *byte);
    }
    escaped_bytes
}

fn unescape_bytes(escaped: &[u8]) -> UsbDataBuffer {
    let mut unescaped: UsbDataBuffer = UsbDataBuffer::new();
    let mut is_escaped = false;

    for &byte in escaped {
        if is_escaped {
            buffer_push!(unescaped, byte);
            is_escaped = false;
        } else if byte == CONTROL_CHAR_DLE {
            is_escaped = true;
        } else {
            buffer_push!(unescaped, byte);
        }
    }
    unescaped
}

fn frame_bytes(payload: &[u8]) -> UsbDataBuffer {
    let mut frame_bytes = UsbDataBuffer::new();
    buffer_push!(frame_bytes, CONTROL_CHAR_STX);
    frame_bytes.extend(escaped_bytes(payload));
    buffer_push!(frame_bytes, CONTROL_CHAR_ETX);
    frame_bytes
}

#[allow(clippy::result_large_err)]
fn unframe_bytes(frame: &[u8]) -> Result<UsbDataBuffer, crate::Error> {
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

impl TryFrom<UsbDataBuffer> for crate::message::Message {
    type Error = crate::Error;
    fn try_from(bytes: UsbDataBuffer) -> Result<Self, Self::Error> {
        let unframed_bytes = unframe_bytes(&bytes)?;
        postcard::from_bytes(&unframed_bytes).map_err(|_| crate::Error::Serialization)
    }
}

impl TryFrom<crate::message::Message> for UsbDataBuffer {
    type Error = crate::Error;
    fn try_from(message: crate::message::Message) -> Result<Self, Self::Error> {
        #[cfg(feature = "std")]
        let message_bytes: UsbDataBuffer =
            postcard::to_allocvec(&message).map_err(|_| crate::Error::Serialization)?;
        #[cfg(not(feature = "std"))]
        let message_bytes =
            postcard::to_vec::<_, 1024>(&message).map_err(|_| crate::Error::Serialization)?;
        let framed_bytes = frame_bytes(&message_bytes);
        Ok(framed_bytes)
    }
}

/// Streaming frame parser that processes a byte stream and emits complete,
/// unframed message payloads.
///
/// Bytes are fed one at a time via [`push_byte`]. When a complete
/// STX…ETX frame is detected, the unframed/unescaped payload is returned.
#[derive(Clone, Debug, Default)]
pub struct MessageStream {
    buffer: UsbDataBuffer,
    is_escaped: bool,
}

impl MessageStream {
    pub fn new() -> Self {
        Self {
            buffer: UsbDataBuffer::new(),
            is_escaped: false,
        }
    }

    /// Feed a single byte. Returns the unframed payload when a complete
    /// frame boundary (ETX) is detected.
    pub fn push_byte(&mut self, byte: u8) -> Option<UsbDataBuffer> {
        // Wait for a frame to start
        if self.buffer.is_empty() {
            if byte == CONTROL_CHAR_STX && !self.is_escaped {
                buffer_push!(self.buffer, byte);
            }
            self.is_escaped = byte == CONTROL_CHAR_DLE;
            return None;
        }

        // Append byte to the accumulation buffer.
        // On no_std targets the buffer is fixed-size; drop the frame on overflow.
        #[cfg(feature = "std")]
        self.buffer.push(byte);
        #[cfg(not(feature = "std"))]
        if self.buffer.push(byte).is_err() {
            self.buffer.clear();
            self.is_escaped = false;
            return None;
        }

        // Detect frame boundary immediately (per-byte ETX check).
        if byte == CONTROL_CHAR_ETX && !self.is_escaped {
            let result = unframe_bytes(&self.buffer).ok();
            self.buffer.clear();
            self.is_escaped = false;
            return result;
        }

        self.is_escaped = byte == CONTROL_CHAR_DLE && !self.is_escaped;
        None
    }

    /// Feed a slice of bytes and collect all complete frame payloads
    /// that were detected.
    #[cfg(feature = "std")]
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<UsbDataBuffer> {
        bytes.iter().filter_map(|&b| self.push_byte(b)).collect()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::message::*;

    /// Create a HostInfo command message and return its framed bytes.
    fn make_host_info_frame() -> Vec<u8> {
        let msg = Message::Command(CommandMessage::root(Command::Host(HostCommand::Info), None));
        let buf: UsbDataBuffer = UsbDataBuffer::try_from(msg).expect("serialize");
        buf
    }

    struct SimResult {
        received: usize,
        elapsed: std::time::Duration,
        total_bytes: usize,
    }

    /// Simulate sending `total` messages where `burst` frames arrive per read().
    fn simulate(burst: usize, total: usize) -> SimResult {
        let frame = make_host_info_frame();
        let mut stream = MessageStream::new();
        let mut received = 0usize;

        let full_batches = total / burst;
        let remainder = total % burst;

        let batch: Vec<u8> = frame
            .iter()
            .cloned()
            .cycle()
            .take(frame.len() * burst)
            .collect();
        let total_bytes = frame.len() * total;

        let start = std::time::Instant::now();

        for _ in 0..full_batches {
            let frames = stream.push_bytes(&batch);
            for payload in &frames {
                if postcard::from_bytes::<Message>(payload).is_ok() {
                    received += 1;
                }
            }
        }

        if remainder > 0 {
            let partial: Vec<u8> = frame
                .iter()
                .cloned()
                .cycle()
                .take(frame.len() * remainder)
                .collect();
            let frames = stream.push_bytes(&partial);
            for payload in &frames {
                if postcard::from_bytes::<Message>(payload).is_ok() {
                    received += 1;
                }
            }
        }

        let elapsed = start.elapsed();
        SimResult {
            received,
            elapsed,
            total_bytes,
        }
    }

    #[test]
    fn single_frame_parses() {
        let frame = make_host_info_frame();
        let mut stream = MessageStream::new();
        let results = stream.push_bytes(&frame);
        assert_eq!(results.len(), 1, "single frame should parse");
        let msg: Message = postcard::from_bytes(&results[0]).expect("deserialize");
        match msg {
            Message::Command(cmd) => match cmd.command {
                Command::Host(HostCommand::Info) => {}
                other => panic!("unexpected command: {:?}", other),
            },
            other => panic!("unexpected message: {:?}", other),
        }
    }

    #[test]
    fn multi_frame_read_recovers_all() {
        // Two frames concatenated in a single read — both must be recovered
        let frame = make_host_info_frame();
        let mut buf = frame.clone();
        buf.extend(&frame);

        let mut stream = MessageStream::new();
        let results = stream.push_bytes(&buf);
        assert_eq!(results.len(), 2, "should recover both frames");
    }

    /// Sweep across burst sizes and verify zero frame loss.
    /// Run with `cargo test -p enody -- packet_drop_sweep --nocapture`
    #[test]
    fn packet_drop_sweep() {
        let burst_sizes: Vec<usize> = vec![1, 2, 3, 5, 10, 20, 50, 100];
        let total_messages = 1000;

        println!("\n--- PACKET DROP SWEEP RESULTS (JSON) ---");

        for &burst in &burst_sizes {
            let result = simulate(burst, total_messages);
            let loss_pct = 100.0 * (1.0 - result.received as f64 / total_messages as f64);
            let elapsed_us = result.elapsed.as_micros() as f64;
            let msgs_per_sec = if elapsed_us > 0.0 {
                result.received as f64 / (elapsed_us / 1_000_000.0)
            } else {
                0.0
            };
            let bytes_per_sec = if elapsed_us > 0.0 {
                result.total_bytes as f64 / (elapsed_us / 1_000_000.0)
            } else {
                0.0
            };
            println!(
                r#"{{"burst_size":{burst},"sent":{total_messages},"received":{},"loss_pct":{loss_pct:.1},"elapsed_us":{elapsed_us:.0},"msgs_per_sec":{msgs_per_sec:.0},"bytes_per_sec":{bytes_per_sec:.0},"total_bytes":{}}}"#,
                result.received, result.total_bytes,
            );
            assert_eq!(
                result.received, total_messages,
                "zero frame loss expected at burst size {burst}"
            );
        }

        println!("--- END SWEEP ---\n");
    }
}
