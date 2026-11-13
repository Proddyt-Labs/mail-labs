// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An internet calendar: the `.ics` file at a web address, read for a calendar that is
//! subscribed to it (`calendar.subscribe`).
//!
//! The address is the owner's, but what comes back is as hostile as a message is: [`fetch`] takes
//! at most [`MAX_BYTES`], gives the whole of it [`PATIENCE`], follows no redirect (the answer of
//! one is an error that says so: the address to subscribe to is the one it leads to), and says
//! what went wrong without the address, which may carry a secret in its path: nothing of it is
//! logged or put in an error.
//!
//! `https://` is spoken to anywhere, by the connections every other web request of this crate
//! uses. Plain `http://` is only for this computer and private networks (the policy is
//! [`mailcraft_model::check_feed_url`]): there is no encryption to protect what is asked for, so
//! nothing may travel beyond the owner's own reach. A host that is a name (a Tailscale name, a
//! name in a home network) is looked up here, and connected to only when every address it has
//! is private; the connection goes to the address that was checked, so a name can't answer one
//! thing when it is checked and another when it is used.

use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use mailcraft_model::{FeedUrlError, check_feed_url, is_private_address};

use crate::NetError;
use crate::http::{self, Request, Url};

/// Largest feed taken, in bytes.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;
/// How long one feed may take, from connecting to its last byte.
pub const PATIENCE: Duration = Duration::from_secs(30);
/// How long a name may take to look up.
const LOOKUP: Duration = Duration::from_secs(5);
/// Most addresses of one host that are looked at.
const MAX_ADDRESSES: usize = 8;

/// Why a feed was not read. Each reads on after "couldn't be updated:" and names no address.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FeedError {
    #[error("{0}")]
    Address(#[from] FeedUrlError),
    #[error("the host is on the internet and plain http:// is only for private networks")]
    NotPrivate,
    #[error("the host's name couldn't be looked up")]
    Lookup,
    #[error("the server couldn't be reached")]
    Unreachable,
    #[error("the server took too long to answer")]
    TooSlow,
    #[error("the file is larger than {} MB", MAX_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("the server redirects to another address: subscribe to that one")]
    Redirect,
    #[error("the server answered {0}")]
    Status(u16),
    #[error("what the server sent isn't an iCalendar file")]
    NotCalendar,
    #[error("the server's answer wasn't one")]
    Garbled,
}

/// Read the iCalendar file at `url`, as the module's notes describe. It blocks, for [`PATIENCE`]
/// at most, so a host calls it on a background thread. The text is whatever the server sent (as
/// UTF-8, a byte order mark dropped) that begins like an iCalendar file; nothing else is checked.
pub fn fetch(url: &str) -> Result<String, FeedError> {
    check_feed_url(url)?;
    let url = address(url)?;
    let headers = [("Accept", "text/calendar, text/plain;q=0.5, */*;q=0.1"), ("Accept-Encoding", "identity")];
    let request = Request { method: "GET", headers: &headers, body: None, patience: PATIENCE, max_body: MAX_BYTES };
    let reply = if url.secure {
        http::request(&url, &request).map_err(|e| reading(&e))?
    } else {
        let mut stream = connect_private(&url)?;
        http::exchange(&mut stream, &url, &request, false).map_err(|e| reading(&e))?.0
    };
    match reply.status {
        200 => text_of(&reply.body),
        301 | 302 | 303 | 307 | 308 => Err(FeedError::Redirect),
        status => Err(FeedError::Status(status)),
    }
}

/// `url` as a [`Url`], plain when it says `http://`.
fn address(url: &str) -> Result<Url, FeedError> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://").ok_or(FeedUrlError::NotAddress)?;
    // `Url` reads an `https://` address to anywhere; plain is `check_feed_url`'s to allow.
    let mut parsed = Url::parse(&format!("https://{rest}")).map_err(|_| FeedUrlError::NotAddress)?;
    parsed.secure = scheme.eq_ignore_ascii_case("https");
    Ok(parsed)
}

fn reading(error: &NetError) -> FeedError {
    if http::is_too_slow(error) {
        FeedError::TooSlow
    } else if http::is_too_long(error) {
        FeedError::TooLarge
    } else if error.is_unreachable() || http::is_closed(error) {
        FeedError::Unreachable
    } else {
        FeedError::Garbled
    }
}

/// The file as text: UTF-8 (what isn't is replaced), without a byte order mark, when it begins as
/// iCalendar does.
fn text_of(body: &[u8]) -> Result<String, FeedError> {
    let body = body.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(body);
    let text = String::from_utf8_lossy(body);
    if text.trim_start().get(..15).is_some_and(|head| head.eq_ignore_ascii_case("BEGIN:VCALENDAR")) {
        Ok(text.into_owned())
    } else {
        Err(FeedError::NotCalendar)
    }
}

/// A connection to the host of a plain `http://` address, made only to addresses that are private,
/// every one the host has.
fn connect_private(url: &Url) -> Result<TcpStream, FeedError> {
    let port = if url.port == 0 { 80 } else { url.port };
    let found = match url.host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => lookup(&url.host, port)?,
    };
    if found.is_empty() {
        return Err(FeedError::Lookup);
    }
    // One address that isn't private refuses them all: a name that points both into the owner's
    // network and at the internet is up to no good.
    if !found.iter().all(|a| is_private_address(a.ip())) {
        return Err(FeedError::NotPrivate);
    }
    let started = Instant::now();
    for address in found.iter().take(MAX_ADDRESSES) {
        let left = PATIENCE.saturating_sub(started.elapsed());
        if left.is_zero() {
            break;
        }
        if let Ok(stream) = TcpStream::connect_timeout(address, left.min(Duration::from_secs(8))) {
            let io = PATIENCE.saturating_sub(started.elapsed()).max(Duration::from_secs(1));
            if stream.set_read_timeout(Some(io)).is_err() || stream.set_write_timeout(Some(io)).is_err() {
                return Err(FeedError::Unreachable);
            }
            return Ok(stream);
        }
    }
    Err(FeedError::Unreachable)
}

/// The addresses of `host`, looked up on a thread that is left behind when the system takes
/// longer than [`LOOKUP`].
fn lookup(host: &str, port: u16) -> Result<Vec<SocketAddr>, FeedError> {
    let host = host.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("calendar feed lookup".into()).spawn(move || {
        let found = (host.as_str(), port).to_socket_addrs().map(|found| found.take(MAX_ADDRESSES * 2).collect::<Vec<_>>()).unwrap_or_default();
        let _ = tx.send(found);
    });
    spawned.map_err(|_| FeedError::Lookup)?;
    rx.recv_timeout(LOOKUP).map_err(|_| FeedError::Lookup)
}
