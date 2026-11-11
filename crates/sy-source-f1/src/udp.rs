// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Receiving the game's UDP packets ([`UdpSource`]).
//!
//! Works on every platform: the game can run on the same PC, under Proton,
//! on another PC or on a console that sends to this PC's address.
//!
//! Every packet received is forwarded unchanged to the configured targets
//! (other telemetry apps), then reduced to the player's car
//! ([`crate::filter`]) before it becomes a raw record (and is spooled).

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sy_model::GameId;
use sy_model::capture::{RawRecord, RawSource, SourceError};
use time::OffsetDateTime;

use crate::decoder::packet_record;
use crate::filter::player_only;
use crate::packets::{Event, Header, id};

/// The game's default port.
pub const DEFAULT_PORT: u16 = 20777;

/// Largest packet of the spec plus room.
const MAX_PACKET: usize = 2048;

/// Settings of the UDP source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpConfig {
    /// Address to listen on, e.g. `127.0.0.1:20777` (this PC only) or
    /// `0.0.0.0:20777` (also consoles and other PCs)
    pub listen: SocketAddr,
    /// Where to forward every packet (other telemetry apps)
    pub forward: Vec<SocketAddr>,
}

/// Reads F1 25 packets from UDP.
#[derive(Debug)]
pub struct UdpSource {
    socket: UdpSocket,
    forward: Vec<SocketAddr>,
    /// A record read by [`wait_for_data`](Self::wait_for_data)
    pending: Option<RawRecord>,
    buf: Vec<u8>,
    /// Session of the last packet
    uid: Option<u64>,
    /// A session started and hasn't ended (`SEND`)
    session_open: Arc<AtomicBool>,
}

impl UdpSource {
    /// Binds the socket.
    pub fn bind(config: &UdpConfig) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(config.listen)?;
        Ok(Self {
            socket,
            forward: config.forward.clone(),
            pending: None,
            buf: vec![0; MAX_PACKET],
            uid: None,
            session_open: Arc::default(),
        })
    }

    /// True while a session runs: from its first packet until the game
    /// says it ended. The game sends nothing while it is paused, so silence
    /// during an open session is a pause, not the end.
    pub fn session_open(&self) -> Arc<AtomicBool> {
        self.session_open.clone()
    }

    fn track_session(&mut self, packet: &[u8]) {
        let Ok(h) = Header::parse(packet) else { return };
        if self.uid != Some(h.session_uid) {
            self.uid = Some(h.session_uid);
            self.session_open.store(true, Ordering::Relaxed);
        }
        if h.packet_id == id::EVENT {
            match Event::parse(packet) {
                Event::SessionStarted => self.session_open.store(true, Ordering::Relaxed),
                Event::SessionEnded => self.session_open.store(false, Ordering::Relaxed),
                _ => {}
            }
        }
    }

    /// The bound address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Waits up to `timeout` for the first F1 packet; it is kept for
    /// [`next_record`](RawSource::next_record). True if one arrived.
    pub fn wait_for_data(&mut self, timeout: Duration) -> Result<bool, SourceError> {
        if self.pending.is_none() {
            self.pending = self.receive(timeout)?;
        }
        Ok(self.pending.is_some())
    }

    /// Receives packets until one is usable or `timeout` passed.
    fn receive(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            // a zero timeout would mean "block forever"
            self.socket
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
            let n = match self.socket.recv_from(&mut self.buf) {
                Ok((n, _)) => n,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Ok(None);
                }
                // Windows reports ICMP "port unreachable" of an earlier
                // send (forwarding to a closed port) on the next receive
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(e) => return Err(e.into()),
            };
            let packet = &self.buf[..n];
            for target in &self.forward {
                if let Err(e) = self.socket.send_to(packet, target) {
                    tracing::debug!(%target, error = %e, "forwarding failed");
                }
            }
            if let Some(filtered) = player_only(packet) {
                self.track_session(&filtered);
                let now_ms = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
                return Ok(Some(packet_record(now_ms, &filtered)));
            }
        }
    }
}

impl RawSource for UdpSource {
    fn game(&self) -> GameId {
        GameId::F1_25
    }

    fn next_record(&mut self, timeout: Duration) -> Result<Option<RawRecord>, SourceError> {
        if let Some(r) = self.pending.take() {
            return Ok(Some(r));
        }
        self.receive(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::KIND_PACKET;
    use crate::packets::{Format, id};
    use crate::sim::PacketBuilder;

    #[test]
    fn receives_filters_and_forwards() {
        let other_app = UdpSocket::bind("127.0.0.1:0").unwrap();
        other_app
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut source = UdpSource::bind(&UdpConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            forward: vec![other_app.local_addr().unwrap()],
        })
        .unwrap();
        let game = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = source.local_addr().unwrap();

        assert!(!source.wait_for_data(Duration::from_millis(50)).unwrap());
        // not F1, then a packet about other cars: forwarded, not recorded
        game.send_to(b"hello", to).unwrap();
        let lobby = PacketBuilder::new(Format::F2026, id::LOBBY_INFO, 1, 0.0, 0);
        game.send_to(&lobby.buf, to).unwrap();
        let mut tel = PacketBuilder::new(Format::F2026, id::CAR_TELEMETRY, 1, 0.0, 2);
        let other = tel.car(5, 0);
        tel.put(other, &[0xEE; 10]);
        game.send_to(&tel.buf, to).unwrap();

        assert!(source.wait_for_data(Duration::from_secs(2)).unwrap());
        let record = source
            .next_record(Duration::from_millis(10))
            .unwrap()
            .unwrap();
        assert_eq!(record.kind, KIND_PACKET);
        let packet = &record.payload[8..];
        assert_eq!(packet.len(), tel.buf.len());
        assert!(packet[other..other + 10].iter().all(|&b| b == 0));

        let mut buf = [0u8; 2048];
        let mut forwarded = Vec::new();
        for _ in 0..3 {
            let (n, _) = other_app.recv_from(&mut buf).unwrap();
            forwarded.push(buf[..n].to_vec());
        }
        assert_eq!(
            forwarded,
            vec![b"hello".to_vec(), lobby.buf, tel.buf.clone()]
        );
        assert!(source.session_open().load(Ordering::Relaxed));

        // the session ends with SEND
        let mut send = PacketBuilder::new(Format::F2026, id::EVENT, 1, 5.0, 2);
        send.put(crate::packets::HEADER_LEN, b"SEND");
        game.send_to(&send.buf, to).unwrap();
        assert!(
            source
                .next_record(Duration::from_secs(2))
                .unwrap()
                .is_some()
        );
        assert!(!source.session_open().load(Ordering::Relaxed));
    }
}
