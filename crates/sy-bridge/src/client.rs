// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The client side, used by the Stintory desktop app on Linux: connect to
//! the bridge, follow the games' states, and read a game's records as a
//! [`RawSource`] ([`BridgeSource`]), so they go through the same spool,
//! decoder and recorder as on Windows.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sy_model::GameId;
use sy_model::capture::{RawRecord, RawSource, SourceError};

use crate::protocol::{BridgeError, Connection, Control, GameState, Message, PROTOCOL};

/// The bridge sends its status every second; this much silence means it's
/// gone (killed with the game, or hanging).
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);

/// What the client knows about the bridge, shared with the UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BridgeView {
    /// Connected to a bridge of this version
    pub bridge_version: Option<String>,
    /// The games the bridge reads for us
    pub games: Vec<GameId>,
    /// Their last reported states
    pub states: HashMap<GameId, GameState>,
}

/// [`BridgeView`] shared between the recording thread and the UI.
pub type SharedView = Arc<Mutex<BridgeView>>;

fn lock(view: &SharedView) -> std::sync::MutexGuard<'_, BridgeView> {
    view.lock().unwrap_or_else(|e| e.into_inner())
}

/// Something the bridge sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A game's state (also already in the [`BridgeView`])
    Status {
        /// Game
        game: GameId,
        /// State
        state: GameState,
    },
    /// A raw record
    Record {
        /// Game
        game: GameId,
        /// Record
        record: RawRecord,
    },
    /// The bridge stopped sending a game's records
    Stopped {
        /// Game
        game: GameId,
    },
}

/// A connection to the bridge.
#[derive(Debug)]
pub struct BridgeClient {
    conn: Connection,
    view: SharedView,
    last_rx: Instant,
}

impl BridgeClient {
    /// Connects to the bridge at `addr` and asks for `games`. `version` is
    /// the client's version (shown in the bridge's messages).
    pub fn connect(
        addr: SocketAddr,
        games: &[GameId],
        version: &str,
        view: SharedView,
        timeout: Duration,
    ) -> Result<Self, BridgeError> {
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        let mut conn = Connection::new(stream)?;
        conn.send_control(Control::Hello {
            protocol: PROTOCOL,
            version: version.to_owned(),
            games: games.to_vec(),
        })?;
        match conn.recv(Duration::from_secs(5))? {
            Some(Message::Control(Control::Hello {
                protocol,
                version,
                games,
            })) => {
                if protocol != PROTOCOL {
                    return Err(BridgeError::Remote(format!(
                        "The Proton bridge ({version}) speaks protocol {protocol}, this Stintory speaks {PROTOCOL}. Use the sy-bridge.exe that comes with this Stintory (Settings → Proton bridge)."
                    )));
                }
                *lock(&view) = BridgeView {
                    bridge_version: Some(version),
                    games,
                    states: HashMap::new(),
                };
                Ok(Self {
                    conn,
                    view,
                    last_rx: Instant::now(),
                })
            }
            Some(Message::Control(Control::Error { message })) => Err(BridgeError::Remote(message)),
            Some(_) => Err(BridgeError::Protocol("expected hello".into())),
            None => Err(BridgeError::Timeout),
        }
    }

    /// Waits up to `timeout` for the next event.
    pub fn next(&mut self, timeout: Duration) -> Result<Option<Event>, BridgeError> {
        let Some(msg) = self.conn.recv(timeout)? else {
            if self.last_rx.elapsed() > HEARTBEAT_TIMEOUT {
                return Err(BridgeError::Timeout);
            }
            return Ok(None);
        };
        self.last_rx = Instant::now();
        match msg {
            Message::Record { game, record } => Ok(Some(Event::Record { game, record })),
            Message::Control(Control::Status { game, state }) => {
                lock(&self.view).states.insert(game, state);
                Ok(Some(Event::Status { game, state }))
            }
            Message::Control(Control::Stopped { game }) => Ok(Some(Event::Stopped { game })),
            Message::Control(Control::Error { message }) => Err(BridgeError::Remote(message)),
            Message::Control(Control::Hello { .. }) => {
                Err(BridgeError::Protocol("unexpected hello".into()))
            }
        }
    }

    /// The records of `game`, starting with `first` (the record that came
    /// with [`Event::Record`]), as a raw source for the pipeline.
    pub fn source(&mut self, game: GameId, first: RawRecord) -> BridgeSource<'_> {
        BridgeSource {
            client: self,
            game,
            first: Some(first),
            ended: Arc::default(),
            error: None,
        }
    }
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        *lock(&self.view) = BridgeView::default();
    }
}

/// One game's records from the bridge, as a [`RawSource`]. It returns
/// `None` once the bridge stopped the recording or the connection broke;
/// [`BridgeSource::ended`] tells the pipeline's idle check.
#[derive(Debug)]
pub struct BridgeSource<'a> {
    client: &'a mut BridgeClient,
    game: GameId,
    first: Option<RawRecord>,
    ended: Arc<AtomicBool>,
    error: Option<BridgeError>,
}

impl BridgeSource<'_> {
    /// Set when the recording is over (bridge said so, or the connection
    /// broke).
    pub fn ended(&self) -> Arc<AtomicBool> {
        self.ended.clone()
    }

    /// Why the connection broke, if it did. The client can't be used after
    /// that.
    pub fn take_error(&mut self) -> Option<BridgeError> {
        self.error.take()
    }
}

impl RawSource for BridgeSource<'_> {
    fn game(&self) -> GameId {
        self.game
    }

    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        if let Some(r) = self.first.take() {
            return Ok(Some(r));
        }
        if self.ended.load(Ordering::Relaxed) {
            std::thread::sleep(timeout.min(Duration::from_millis(10)));
            return Ok(None);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.client.next(left) {
                Ok(Some(Event::Record { game, record })) if game == self.game => {
                    return Ok(Some(record));
                }
                Ok(Some(Event::Record { game, .. })) => {
                    tracing::debug!(%game, "ignoring a record of another game");
                }
                Ok(Some(Event::Stopped { game })) if game == self.game => {
                    self.ended.store(true, Ordering::Relaxed);
                    return Ok(None);
                }
                Ok(Some(_)) => {}
                Ok(None) => return Ok(None),
                Err(e) => {
                    // not a source error: that would mark the recording
                    // failed. What arrived is kept; the recording ends.
                    self.error = Some(e);
                    self.ended.store(true, Ordering::Relaxed);
                    return Ok(None);
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
}
