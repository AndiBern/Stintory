// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bridge side: wait for the Stintory client on localhost, then watch
//! the games and stream their records while the player drives.
//!
//! The games are behind the [`Game`] trait, so the same loop runs with the
//! real shared-memory readers (`sy-bridge.exe` on Windows/Wine) and with
//! simulated games ([`crate::sim`], tests and `--simulate-lmu`).
//!
//! Nothing is read while no client is connected, and every connection
//! starts its readers afresh: a client that connects in the middle of a
//! session gets the records a reader sends first (iRacing's variable table,
//! the session info), just as if it had read the game itself.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sy_model::GameId;
use sy_model::capture::{RawRecord, SourceError};

use crate::protocol::{BridgeError, Connection, Control, GameState, Message, PROTOCOL};

/// A game the bridge can read.
pub trait Game: Send {
    /// Which game.
    fn id(&self) -> GameId;
    /// What the game is doing (cheap; called about once a second).
    fn probe(&mut self) -> GameState;
    /// Starts reading records; called when [`Game::probe`] said
    /// [`GameState::Driving`].
    fn open(&mut self) -> Box<dyn Feed>;
}

/// A running reader of a game's records.
pub trait Feed: Send {
    /// Waits up to `timeout` for the next record (already player-only).
    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError>;
    /// The player drives right now.
    fn driving(&self) -> bool;
    /// No record arrived for `idle`: is the recording over? (The game sends
    /// nothing while paused; then the answer depends on [`Feed::driving`].)
    fn done(&self, idle: Duration) -> bool;
}

/// How often the bridge reports every game's state.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);
/// How long a new client has to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// The bridge: a listener and the games it can read.
pub struct Bridge {
    listener: TcpListener,
    games: Vec<Box<dyn Game>>,
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("listener", &self.listener)
            .field(
                "games",
                &self.games.iter().map(|g| g.id()).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Bridge {
    /// Listens on `addr` (normally `127.0.0.1:20790`).
    pub fn bind(addr: SocketAddr, games: Vec<Box<dyn Game>>) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            games,
            stop: Arc::default(),
        })
    }

    /// The address it listens on.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Set to stop [`Bridge::run`] (checked a few times a second).
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Serves clients, one at a time, until stopped.
    pub fn run(&mut self) {
        while !self.stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, peer)) => {
                    tracing::info!(%peer, "Stintory connected");
                    match self.serve(stream) {
                        Ok(()) | Err(BridgeError::Closed) => {
                            tracing::info!(%peer, "Stintory disconnected");
                        }
                        // how Windows/Wine reports a closed connection
                        Err(BridgeError::Io(e))
                            if matches!(
                                e.kind(),
                                io::ErrorKind::ConnectionReset
                                    | io::ErrorKind::ConnectionAborted
                                    | io::ErrorKind::BrokenPipe
                            ) =>
                        {
                            tracing::info!(%peer, "Stintory disconnected");
                        }
                        Err(e) => tracing::warn!(%peer, error = %e, "connection ended"),
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accepting a connection failed");
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }

    fn serve(&mut self, stream: TcpStream) -> Result<(), BridgeError> {
        let mut conn = Connection::new(stream)?;
        let wanted = match conn.recv(HELLO_TIMEOUT)? {
            Some(Message::Control(Control::Hello {
                protocol, games, ..
            })) if protocol == PROTOCOL => games,
            Some(Message::Control(Control::Hello {
                protocol, version, ..
            })) => {
                let message = format!(
                    "This bridge ({}) speaks protocol {PROTOCOL}, Stintory {version} speaks {protocol}. Use the sy-bridge.exe that comes with your Stintory (Settings → Proton bridge).",
                    env!("CARGO_PKG_VERSION")
                );
                let _ = conn.send_control(Control::Error {
                    message: message.clone(),
                });
                return Err(BridgeError::Remote(message));
            }
            Some(_) => return Err(BridgeError::Protocol("expected hello".into())),
            None => return Err(BridgeError::Timeout),
        };
        let games: Vec<&mut Box<dyn Game>> = self
            .games
            .iter_mut()
            .filter(|g| wanted.contains(&g.id()))
            .collect();
        conn.send_control(Control::Hello {
            protocol: PROTOCOL,
            version: env!("CARGO_PKG_VERSION").into(),
            games: games.iter().map(|g| g.id()).collect(),
        })?;
        let mut session = Session {
            conn,
            listener: &self.listener,
            stop: &self.stop,
        };
        session.watch(games)
    }
}

/// One connected client.
struct Session<'a> {
    conn: Connection,
    listener: &'a TcpListener,
    stop: &'a AtomicBool,
}

impl Session<'_> {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Turns away other clients: only one may record.
    fn reject_others(&self) {
        while let Ok((stream, peer)) = self.listener.accept() {
            tracing::info!(%peer, "turned away a second connection");
            if let Ok(mut c) = Connection::new(stream) {
                let _ = c.send_control(Control::Error {
                    message: "The Proton bridge is already connected to another Stintory.".into(),
                });
            }
        }
    }

    /// Waits up to `timeout`; notices when the client goes away.
    fn wait(&mut self, timeout: Duration) -> Result<(), BridgeError> {
        match self.conn.recv(timeout)? {
            Some(Message::Control(Control::Error { message })) => Err(BridgeError::Remote(message)),
            // the client has nothing else to say yet
            _ => Ok(()),
        }
    }

    fn status(&mut self, game: GameId, state: GameState) -> Result<(), BridgeError> {
        self.conn.send_control(Control::Status { game, state })
    }

    fn watch(&mut self, mut games: Vec<&mut Box<dyn Game>>) -> Result<(), BridgeError> {
        while !self.stopping() {
            let tick = Instant::now();
            for game in games.iter_mut() {
                let state = game.probe();
                self.status(game.id(), state)?;
                if state == GameState::Driving {
                    self.forward(&mut ***game)?;
                }
            }
            self.reject_others();
            self.wait(STATUS_INTERVAL.saturating_sub(tick.elapsed()))?;
        }
        Ok(())
    }

    /// Streams a game's records until its feed is done.
    fn forward(&mut self, game: &mut dyn Game) -> Result<(), BridgeError> {
        let id = game.id();
        tracing::info!(game = %id, "player on track, sending records");
        let mut feed = game.open();
        let mut idle_since: Option<Instant> = None;
        let mut last_status = Instant::now();
        let mut records = 0u64;
        while !self.stopping() {
            match feed.next_record(Duration::from_millis(100)) {
                Ok(Some(record)) => {
                    idle_since = None;
                    records += 1;
                    self.conn.send(&Message::Record { game: id, record })?;
                }
                Ok(None) => {
                    if feed.done(idle_since.get_or_insert_with(Instant::now).elapsed()) {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(game = %id, error = %e, "reading the game failed");
                    std::thread::sleep(Duration::from_secs(1));
                    break;
                }
            }
            if last_status.elapsed() >= STATUS_INTERVAL {
                last_status = Instant::now();
                let state = if feed.driving() {
                    GameState::Driving
                } else {
                    GameState::Idle
                };
                self.status(id, state)?;
                self.reject_others();
            }
        }
        tracing::info!(game = %id, records, "recording over");
        self.conn.send_control(Control::Stopped { game: id })
    }
}
