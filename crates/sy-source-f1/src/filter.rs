// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Removes everything about other cars and drivers from a packet before it
//! is spooled to disk (docs/ARCHITECTURE.md §5.2).
//!
//! - Per-car arrays: every entry except the player's is zeroed (also the
//!   second player's in split screen: that is another person).
//! - Packets about one other car (session history, tyre sets) are dropped.
//! - Packets that are mostly about others (final classification, lobby,
//!   lap positions, time trial with the rival's lap) are dropped; Stintory
//!   doesn't use them.
//! - Events: only those that are not about a car are kept (session start
//!   and end, flashback, chequered flag, red flag, lights out, safety car),
//!   and the player's own penalties, without the other car involved.
//! - Packets of unknown formats, ids or sizes are dropped.

use crate::packets::{Event, HEADER_LEN, Header, SessionHistory, car_array, id};

/// Event codes that are kept (none of them names a car).
const KEPT_EVENTS: [&[u8; 4]; 7] = [
    b"SSTA", b"SEND", b"FLBK", b"CHQF", b"RDFL", b"LGOT", b"SCAR",
];

/// The packet reduced to the player's car, or `None` if it is dropped.
pub fn player_only(packet: &[u8]) -> Option<Vec<u8>> {
    let h = Header::parse(packet).ok()?;
    match h.packet_id {
        id::FINAL_CLASSIFICATION
        | id::LOBBY_INFO
        | id::TYRE_SETS
        | id::TIME_TRIAL
        | id::LAP_POSITIONS => None,
        id::SESSION_HISTORY => {
            (SessionHistory::car(packet) == h.player_car).then(|| packet.to_vec())
        }
        id::EVENT => match Event::parse(packet) {
            Event::Penalty { car, .. } if car == h.player_car => {
                let mut out = packet.to_vec();
                out[HEADER_LEN + 4 + 3] = 255; // the other car involved
                Some(out)
            }
            _ => KEPT_EVENTS
                .contains(&&Event::code(packet))
                .then(|| packet.to_vec()),
        },
        // no data about other cars
        id::SESSION | id::MOTION_EX => Some(packet.to_vec()),
        other => {
            let (offset, stride) = car_array(h.format, other)?;
            let mut out = packet.to_vec();
            let player = usize::from(h.player_car);
            for car in (0..h.format.cars()).filter(|&c| c != player) {
                let start = offset + car * stride;
                out[start..start + stride].fill(0);
            }
            if other == id::PARTICIPANTS {
                // number of active cars: the player only
                out[HEADER_LEN] = 1;
            }
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{Format, packet_size};
    use crate::sim::PacketBuilder;

    #[test]
    fn keeps_only_the_players_car() {
        for format in [Format::F2025, Format::F2026] {
            for packet_id in [
                id::MOTION,
                id::LAP_DATA,
                id::PARTICIPANTS,
                id::CAR_SETUPS,
                id::CAR_TELEMETRY,
                id::CAR_STATUS,
                id::CAR_DAMAGE,
            ] {
                let mut p = PacketBuilder::new(format, packet_id, 9, 1.0, 3);
                let (offset, _) = car_array(format, packet_id).unwrap();
                p.buf[offset..].fill(0xAB);
                let out = player_only(&p.buf).unwrap();
                assert_eq!(out.len(), packet_size(format, packet_id).unwrap());
                assert_eq!(out[..HEADER_LEN], p.buf[..HEADER_LEN]);
                for car in 0..format.cars() as u8 {
                    let (at, _) = (p.car(car, 0), ());
                    let (_, stride) = car_array(format, packet_id).unwrap();
                    let entry = &out[at..at + stride];
                    if car == 3 {
                        assert!(entry.iter().all(|&b| b == 0xAB), "{format} {packet_id}");
                    } else {
                        assert!(
                            entry.iter().all(|&b| b == 0),
                            "{format} {packet_id} car {car}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn drops_packets_about_others() {
        for packet_id in [
            id::FINAL_CLASSIFICATION,
            id::LOBBY_INFO,
            id::TYRE_SETS,
            id::TIME_TRIAL,
            id::LAP_POSITIONS,
        ] {
            let p = PacketBuilder::new(Format::F2026, packet_id, 1, 0.0, 0);
            assert!(player_only(&p.buf).is_none(), "{packet_id}");
        }
        let mut history = PacketBuilder::new(Format::F2026, id::SESSION_HISTORY, 1, 0.0, 4);
        history.buf[HEADER_LEN] = 5;
        assert!(player_only(&history.buf).is_none(), "another car's history");
        history.buf[HEADER_LEN] = 4;
        assert!(player_only(&history.buf).is_some());

        // the player is car 4; the details below name car 0
        let mut event = PacketBuilder::new(Format::F2026, id::EVENT, 1, 0.0, 4);
        for (code, kept) in [
            (b"FLBK", true),
            (b"SEND", true),
            (b"OVTK", false),
            (b"COLL", false),
            (b"PENA", false), // another car's
            (b"FTLP", false),
            (b"BUTN", false),
        ] {
            event.put(HEADER_LEN, code);
            assert_eq!(player_only(&event.buf).is_some(), kept);
        }
        // the player's own penalty is kept, without the other car
        event
            .put(HEADER_LEN, b"PENA")
            .put(HEADER_LEN + 4, &[10, 26, 4, 7, 0, 3, 0]);
        let kept = player_only(&event.buf).unwrap();
        assert_eq!(kept[HEADER_LEN + 4 + 2], 4);
        assert_eq!(kept[HEADER_LEN + 4 + 3], 255, "other car removed");
        assert_eq!(
            Event::parse(&kept),
            Event::Penalty {
                penalty_type: 10,
                infringement: 26,
                car: 4,
                lap: 3
            }
        );
        assert!(player_only(b"not a packet").is_none());
        let short = PacketBuilder::new(Format::F2026, id::CAR_TELEMETRY, 1, 0.0, 0);
        assert!(player_only(&short.buf[..500]).is_none());
    }
}
