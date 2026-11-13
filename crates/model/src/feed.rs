// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An internet calendar: a calendar whose events are those of an `.ics` file at a web address,
//! read again from time to time and never changed here.
//!
//! This is the data and the policy only; the engine merges what was read into the calendar, and
//! the host (`mailcraft-net`) reads the address. What may be fetched is decided here, so that
//! the engine can refuse an address before anything is asked and the host can check the very
//! addresses it connects to with the same rule: `https://` anywhere, plain `http://` only to a
//! host on this computer or on a private network (RFC 1918, loopback, carrier-grade NAT, which
//! is also where Tailscale puts its addresses), where nothing travels beyond the owner's reach.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

/// Longest address kept.
pub const MAX_FEED_URL: usize = 2_000;
/// Most internet calendars one mailbox may subscribe to.
pub const MAX_FEEDS: usize = 100;
/// Most events one feed may bring in; what is beyond is left out.
pub const MAX_FEED_EVENTS: usize = 20_000;
/// Longest error text kept.
pub const MAX_FEED_ERROR: usize = 300;
/// How often a feed is read again while the program runs, in seconds.
pub const FEED_EVERY_SECS: i64 = 3 * 60 * 60;

/// What a subscribed calendar remembers of where it comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscription {
    pub url: String,
    /// When the events were last replaced by the feed's (seconds since 1970); 0: never.
    pub last_refresh: i64,
    /// When the feed was last asked for, whatever came of it (seconds); 0: never.
    pub last_attempt: i64,
    /// Why the last attempt failed; empty when it didn't.
    pub last_error: String,
    /// The feed is to be read at the next chance (a new subscription, or the owner asked).
    pub stale: bool,
}

impl Subscription {
    /// Whether the feed should be read at `now` (seconds): asked for, never tried, or not tried
    /// for [`FEED_EVERY_SECS`] (a failure is waited out as long as a success).
    pub fn due(&self, now: i64) -> bool {
        self.stale || self.last_attempt == 0 || now.saturating_sub(self.last_attempt) >= FEED_EVERY_SECS
    }
}

/// Why an address can't be subscribed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FeedUrlError {
    #[error("the address isn't a web address (https://…)")]
    NotAddress,
    #[error("a web address with a user name or password in it isn't taken")]
    Credentials,
    #[error("plain http:// is only for this computer and private networks; use https:// for the internet")]
    PlainHttp,
}

/// Whether `ip` is one that plain `http://` may be spoken to: this computer (127/8, ::1), a
/// private network (10/8, 172.16/12, 192.168/16, fc00::/7), carrier-grade NAT (100.64/10, where
/// Tailscale's addresses are) or a link-local address. An IPv6 address that carries an IPv4 one
/// is judged by it.
pub fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => private_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(inner) => private_v4(inner),
            None => private_v6(ip),
        },
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b))
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
}

fn private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments().first().copied().unwrap_or(0);
    ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
}

/// Whether the address is plain `http://`, and its host without brackets.
fn split(url: &str) -> Result<(bool, &str), FeedUrlError> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_FEED_URL || !url.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(FeedUrlError::NotAddress);
    }
    let (plain, rest) = match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => (false, rest),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => (true, rest),
        _ => return Err(FeedUrlError::NotAddress),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(FeedUrlError::Credentials);
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']').map(|(host, _)| host).ok_or(FeedUrlError::NotAddress)?,
        None => authority.split(':').next().unwrap_or(""),
    };
    if host.is_empty() {
        return Err(FeedUrlError::NotAddress);
    }
    Ok((plain, host))
}

/// The host of a feed's address, when the address may be subscribed to by what its text says:
/// `https://` to any host; `http://` to an address in [`is_private_address`], or to a name,
/// which whoever looks it up must check again: every address it has has to be private.
pub fn check_feed_url(url: &str) -> Result<(), FeedUrlError> {
    let (plain, host) = split(url)?;
    if !plain {
        return Ok(());
    }
    match host.parse::<IpAddr>() {
        Ok(ip) if is_private_address(ip) => Ok(()),
        Ok(_) => Err(FeedUrlError::PlainHttp),
        Err(_) => Ok(()),
    }
}
