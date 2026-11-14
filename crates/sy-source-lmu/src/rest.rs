// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! LMU's local REST API (`http://127.0.0.1:6397`).
//!
//! Stintory only ever sends `GET` requests, and only to the endpoint listed
//! here: the game has many endpoints that change things (garage, pit menu,
//! session control), and none of them may be called (docs/ARCHITECTURE.md §13).
//! Not even every `GET` is harmless (`/rest/options/resetVRView`,
//! `/rest/multiplayer/join`, `/rest/profile/getAuthSessionTicket`, …), so
//! new endpoints are added here one by one, after looking at what they do.
//!
//! - [`game_running`]: the API answers while LMU runs, also when the shared
//!   memory is off. That's how the app tells "LMU isn't running" from
//!   "LMU runs, but *Enable Plugins* is off".
//! - [`SetupPoller`]: reads the player's car setup
//!   (`GET /rest/garage/getPlayerGarageData`) every few seconds, off the
//!   telemetry thread.
//!
//! A tiny HTTP/1.1 client (`Connection: close`, `Content-Length` or chunked
//! bodies) is enough for a server on the same PC, so no HTTP library is
//! needed.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Where the game's REST API listens.
pub const REST_ADDR: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::LOCALHOST,
    6397,
));

/// The player's garage data: the current car setup.
pub const SETUP_PATH: &str = "/rest/garage/getPlayerGarageData";

/// Largest response body accepted.
const MAX_BODY: usize = 8 * 1024 * 1024;

/// True if something answers on the game's REST port.
pub fn game_running(addr: SocketAddr, timeout: Duration) -> bool {
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

/// `GET path` on `addr`; returns the status code and the body.
pub fn get(addr: SocketAddr, path: &str, timeout: Duration) -> io::Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    let mut r = BufReader::new(stream);

    let mut line = String::new();
    r.read_line(&mut line)?;
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("no HTTP status line"))?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Err(bad("headers cut off"));
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.parse().map_err(|_| bad("bad Content-Length"))?);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.to_ascii_lowercase().contains("chunked");
            }
        }
    }

    let mut body = Vec::new();
    if chunked {
        loop {
            line.clear();
            r.read_line(&mut line)?;
            let size_text = line.trim().split(';').next().unwrap_or_default();
            let size = usize::from_str_radix(size_text, 16).map_err(|_| bad("bad chunk size"))?;
            if size == 0 {
                break;
            }
            if body.len() + size > MAX_BODY {
                return Err(bad("response too large"));
            }
            let start = body.len();
            body.resize(start + size, 0);
            r.read_exact(&mut body[start..])?;
            let mut crlf = [0u8; 2];
            r.read_exact(&mut crlf)?;
        }
    } else if let Some(len) = length {
        if len > MAX_BODY {
            return Err(bad("response too large"));
        }
        body.resize(len, 0);
        r.read_exact(&mut body)?;
    } else {
        r.take(MAX_BODY as u64 + 1).read_to_end(&mut body)?;
        if body.len() > MAX_BODY {
            return Err(bad("response too large"));
        }
    }
    Ok((status, body))
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Reads the player's car setup in a background thread and reports it when
/// it changed. Stops when dropped.
#[derive(Debug)]
pub struct SetupPoller {
    rx: Receiver<Vec<u8>>,
    _stop: Sender<()>,
}

impl SetupPoller {
    /// Polls [`SETUP_PATH`] on `addr` every `interval`.
    pub fn start(addr: SocketAddr, interval: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("lmu-rest".into())
            .spawn(move || {
                let mut last: Option<Vec<u8>> = None;
                loop {
                    match get(addr, SETUP_PATH, Duration::from_secs(2)) {
                        Ok((200, body))
                            if last.as_ref() != Some(&body)
                                && serde_json::from_slice::<serde_json::Value>(&body)
                                    .is_ok_and(|v| !v.is_null()) =>
                        {
                            if tx.send(body.clone()).is_err() {
                                return;
                            }
                            last = Some(body);
                        }
                        Ok(_) => {}
                        Err(e) => tracing::trace!(error = %e, "LMU REST API not reachable"),
                    }
                    match stop_rx.recv_timeout(interval) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => return, // dropped
                    }
                }
            })
            .expect("spawn LMU REST thread");
        Self { rx, _stop: stop_tx }
    }

    /// The setup, if it changed since the last call.
    pub fn changed(&self) -> Option<Vec<u8>> {
        self.rx.try_iter().last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Answers one request per response with the given raw HTTP response.
    fn server(responses: Vec<&'static str>) -> (SocketAddr, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (mut s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut l = String::new();
                while r.read_line(&mut l).unwrap() > 2 {
                    l.clear();
                }
                requests.push(first.trim().to_owned());
                s.write_all(response.as_bytes()).unwrap();
            }
            requests
        });
        (addr, handle)
    }

    #[test]
    fn reads_content_length_and_chunked_bodies() {
        let (addr, h) = server(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"a\":[1,2]}",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"b\"\r\n3\r\n:1}\r\n0\r\n\r\n",
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        ]);
        let t = Duration::from_secs(2);
        assert_eq!(
            get(addr, "/x", t).unwrap(),
            (200, b"{\"a\":[1,2]}".to_vec())
        );
        assert_eq!(get(addr, "/y", t).unwrap(), (200, b"{\"b\":1}".to_vec()));
        assert_eq!(get(addr, "/z", t).unwrap().0, 404);
        let requests = h.join().unwrap();
        assert_eq!(
            requests,
            ["GET /x HTTP/1.1", "GET /y HTTP/1.1", "GET /z HTTP/1.1"]
        );
    }

    #[test]
    fn setup_poller_reports_changes_only() {
        let (addr, h) = server(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}",
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}",
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":2}",
        ]);
        let poller = SetupPoller::start(addr, Duration::from_millis(20));
        let requests = h.join().unwrap();
        assert!(
            requests
                .iter()
                .all(|r| r == &format!("GET {SETUP_PATH} HTTP/1.1"))
        );
        std::thread::sleep(Duration::from_millis(50));
        let mut seen = Vec::new();
        while let Ok(s) = poller.rx.try_recv() {
            seen.push(s);
        }
        assert_eq!(seen, vec![b"{\"a\":1}".to_vec(), b"{\"a\":2}".to_vec()]);
    }

    #[test]
    fn game_running_needs_a_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(game_running(addr, Duration::from_millis(500)));
        drop(listener);
        assert!(!game_running(addr, Duration::from_millis(500)));
    }
}
