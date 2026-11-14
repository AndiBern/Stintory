// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bridge protocol: messages between `sy-bridge.exe` (inside the Wine
//! prefix) and the Stintory client, over a localhost TCP connection.
//!
//! Every message is one frame:
//!
//! ```text
//! length: u32 LE (of everything after it) | type: u8 | body
//! ```
//!
//! | type | body |
//! |---|---|
//! | [`TYPE_CONTROL`] | a [`Control`] message as JSON |
//! | [`TYPE_RECORD`] | game name length `u8`, game name (`GameId::as_str`), record kind `u8`, payload |
//!
//! The conversation:
//!
//! 1. the client connects and sends [`Control::Hello`] with the games it
//!    wants;
//! 2. the bridge answers with its own `Hello` (the games it reads of those),
//!    or [`Control::Error`] and closes (another protocol version, another
//!    client already connected);
//! 3. the bridge sends [`Control::Status`] for every game at least once a
//!    second (this is also the heartbeat), the raw records of a game while
//!    the player drives, and [`Control::Stopped`] when that recording ended.
//!
//! Records are exactly what the game's `RawSource` returns on Windows:
//! already filtered to the player's car (docs/ARCHITECTURE.md §5.2). The client
//! spools and decodes them as if it had read the game itself.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sy_model::GameId;
use sy_model::capture::RawRecord;

/// Version of this protocol. Bridge and client must speak the same one.
pub const PROTOCOL: u32 = 1;
/// Default TCP port of the bridge (on 127.0.0.1).
pub const DEFAULT_PORT: u16 = 20790;
/// Largest frame accepted.
pub const MAX_FRAME: usize = 16 << 20;
/// Frame type: [`Control`] as JSON.
pub const TYPE_CONTROL: u8 = 1;
/// Frame type: a raw record.
pub const TYPE_RECORD: u8 = 2;

/// What the bridge sees of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameState {
    /// The game isn't running (in this Wine prefix).
    NotRunning,
    /// The game runs but shares no telemetry (LMU: *Enable Plugins* off).
    NoSharedMemory,
    /// The shared memory has a layout this version doesn't know.
    UnknownLayout,
    /// Connected to the game; the player isn't driving.
    Idle,
    /// The player drives; records are sent.
    Driving,
}

/// Control messages (JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    /// First message in both directions.
    Hello {
        /// [`PROTOCOL`] of the sender
        protocol: u32,
        /// Version of the sending program
        version: String,
        /// Client: the games to read. Bridge: the games it reads.
        games: Vec<GameId>,
    },
    /// Bridge → client: the state of a game (at least once a second).
    Status {
        /// Game
        game: GameId,
        /// State
        state: GameState,
    },
    /// Bridge → client: the bridge stopped reading the game's records; the
    /// recording is over.
    Stopped {
        /// Game
        game: GameId,
    },
    /// Either side: something is wrong; the connection is closed after it.
    Error {
        /// Human-readable message
        message: String,
    },
}

/// One message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A control message
    Control(Control),
    /// A raw record of a game
    Record {
        /// Game the record belongs to
        game: GameId,
        /// The record
        record: RawRecord,
    },
}

/// Errors of the bridge connection.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// Socket error
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The other side sent something that isn't this protocol
    #[error("invalid bridge message: {0}")]
    Protocol(String),
    /// The other side closed the connection
    #[error("the connection was closed")]
    Closed,
    /// Nothing arrived for too long
    #[error("the other side stopped answering")]
    Timeout,
    /// The other side reported an error
    #[error("{0}")]
    Remote(String),
}

/// Encodes a message as one frame.
pub fn encode(msg: &Message) -> Vec<u8> {
    let mut out = vec![0u8; 4];
    match msg {
        Message::Control(c) => {
            out.push(TYPE_CONTROL);
            serde_json::to_writer(&mut out, c).expect("control messages serialize");
        }
        Message::Record { game, record } => {
            out.push(TYPE_RECORD);
            let name = game.as_str().as_bytes();
            out.push(u8::try_from(name.len()).expect("short game name"));
            out.extend_from_slice(name);
            out.push(record.kind);
            out.extend_from_slice(&record.payload);
        }
    }
    let len = u32::try_from(out.len() - 4).expect("frame fits u32");
    out[..4].copy_from_slice(&len.to_le_bytes());
    out
}

fn decode(frame: &[u8]) -> Result<Message, BridgeError> {
    let bad = |m: &str| BridgeError::Protocol(m.to_owned());
    let (&kind, body) = frame.split_first().ok_or_else(|| bad("empty frame"))?;
    match kind {
        TYPE_CONTROL => serde_json::from_slice(body)
            .map(Message::Control)
            .map_err(|e| BridgeError::Protocol(format!("control message: {e}"))),
        TYPE_RECORD => {
            let (&n, rest) = body.split_first().ok_or_else(|| bad("record: no game"))?;
            let n = usize::from(n);
            if rest.len() < n + 1 {
                return Err(bad("record: too short"));
            }
            let name = std::str::from_utf8(&rest[..n]).map_err(|_| bad("record: game name"))?;
            let game = name
                .parse::<GameId>()
                .map_err(|_| BridgeError::Protocol(format!("record: unknown game {name:?}")))?;
            Ok(Message::Record {
                game,
                record: RawRecord {
                    kind: rest[n],
                    payload: rest[n + 1..].to_vec(),
                },
            })
        }
        other => Err(BridgeError::Protocol(format!("unknown frame type {other}"))),
    }
}

/// Collects bytes from the socket and splits them into messages.
#[derive(Debug, Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Adds received bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete message, if one was received.
    pub fn next_message(&mut self) -> Result<Option<Message>, BridgeError> {
        let Some(head) = self.buf.get(..4) else {
            return Ok(None);
        };
        let len = u32::from_le_bytes(head.try_into().expect("4 bytes")) as usize;
        if len > MAX_FRAME {
            return Err(BridgeError::Protocol(format!("frame of {len} bytes")));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let msg = decode(&self.buf[4..4 + len]);
        self.buf.drain(..4 + len);
        msg.map(Some)
    }
}

/// A framed connection.
#[derive(Debug)]
pub struct Connection {
    stream: TcpStream,
    reader: FrameReader,
}

impl Connection {
    /// Wraps a connected stream (blocking mode; writes time out after 5 s).
    pub fn new(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(false)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self {
            stream,
            reader: FrameReader::default(),
        })
    }

    /// Sends a message.
    pub fn send(&mut self, msg: &Message) -> Result<(), BridgeError> {
        self.stream.write_all(&encode(msg))?;
        Ok(())
    }

    /// Sends a control message.
    pub fn send_control(&mut self, control: Control) -> Result<(), BridgeError> {
        self.send(&Message::Control(control))
    }

    /// Waits up to `timeout` for the next message.
    pub fn recv(&mut self, timeout: Duration) -> Result<Option<Message>, BridgeError> {
        let deadline = Instant::now() + timeout;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            if let Some(msg) = self.reader.next_message()? {
                return Ok(Some(msg));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            // a zero timeout means "block forever" to the socket
            self.stream
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(BridgeError::Closed),
                Ok(n) => self.reader.extend(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: Message) {
        let mut r = FrameReader::default();
        let bytes = encode(&msg);
        // byte by byte: a frame is only complete at the end
        for (i, b) in bytes.iter().enumerate() {
            assert_eq!(r.next_message().unwrap(), None, "byte {i}");
            r.extend(&[*b]);
        }
        assert_eq!(r.next_message().unwrap(), Some(msg));
        assert_eq!(r.next_message().unwrap(), None);
    }

    #[test]
    fn messages_roundtrip() {
        roundtrip(Message::Control(Control::Hello {
            protocol: PROTOCOL,
            version: "0.1.0".into(),
            games: vec![GameId::Lmu, GameId::Iracing],
        }));
        roundtrip(Message::Control(Control::Status {
            game: GameId::Lmu,
            state: GameState::NoSharedMemory,
        }));
        roundtrip(Message::Control(Control::Stopped { game: GameId::Lmu }));
        roundtrip(Message::Control(Control::Error {
            message: "nope".into(),
        }));
        roundtrip(Message::Record {
            game: GameId::Lmu,
            record: RawRecord {
                kind: 2,
                payload: (0..=255).collect(),
            },
        });
        roundtrip(Message::Record {
            game: GameId::Iracing,
            record: RawRecord {
                kind: 1,
                payload: Vec::new(),
            },
        });
    }

    #[test]
    fn control_messages_are_readable_json() {
        let bytes = encode(&Message::Control(Control::Status {
            game: GameId::Lmu,
            state: GameState::Driving,
        }));
        assert_eq!(bytes[4], TYPE_CONTROL);
        assert_eq!(
            std::str::from_utf8(&bytes[5..]).unwrap(),
            r#"{"type":"status","game":"lmu","state":"driving"}"#
        );
    }

    #[test]
    fn several_frames_in_one_read() {
        let mut r = FrameReader::default();
        let a = Message::Control(Control::Stopped { game: GameId::Lmu });
        let b = Message::Control(Control::Stopped {
            game: GameId::Iracing,
        });
        let mut bytes = encode(&a);
        bytes.extend(encode(&b));
        r.extend(&bytes);
        assert_eq!(r.next_message().unwrap(), Some(a));
        assert_eq!(r.next_message().unwrap(), Some(b));
        assert_eq!(r.next_message().unwrap(), None);
    }

    #[test]
    fn garbage_is_rejected() {
        let mut r = FrameReader::default();
        r.extend(&u32::MAX.to_le_bytes());
        assert!(r.next_message().is_err(), "too large");

        for frame in [
            vec![9u8],                           // unknown type
            vec![TYPE_CONTROL, b'{'],            // bad JSON
            vec![TYPE_RECORD, 3, b'l', b'm'],    // truncated
            vec![TYPE_RECORD, 2, b'x', b'y', 1], // unknown game
        ] {
            let mut r = FrameReader::default();
            r.extend(&(frame.len() as u32).to_le_bytes());
            r.extend(&frame);
            assert!(r.next_message().is_err(), "{frame:?}");
        }
    }
}
