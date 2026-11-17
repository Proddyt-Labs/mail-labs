// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the sign-in Google Keep takes, which is the Android app's (the protocol
//! `gpsoauth` speaks; written here from its public description).
//!
//! 1. **Once.** The owner signs in on Google's own page (`accounts.google.com/EmbeddedSetup`) and
//!    pastes the `oauth_token` cookie that page sets. [`exchange_token`] trades it, at Google's
//!    Android sign-in server, for a *master token* (`aas_et/…`): a lasting grant for that Google
//!    account. It goes to the system's credential store and nowhere else.
//! 2. **Every sync.** [`access_token`] trades the master token for an access token for Keep alone
//!    (about an hour), held in memory for that one sync.
//!
//! **Secrets.** The cookie, the master token and the access token are [`Secret`]s: no `Debug` or
//! `Display` shows them, they are overwritten when dropped, and the cookie's buffer is overwritten
//! as soon as it was traded. Google's answers carry them, so an answer is never put in an error,
//! a log or a message: what a refusal says is one of the fixed sentences here, chosen by the
//! status and by Google's own error code. The master token goes to Google's Android sign-in
//! server and to no other place ([`may_go_to`]).

use std::time::Duration;

use mailcraft_proto::ProtoError;
use zeroize::Zeroize;

use crate::NetError;
use crate::http::{self, Request, Url};

/// Google's Android sign-in server.
const AUTH: &str = "https://android.clients.google.com";
const AUTH_HOST: &str = "android.clients.google.com";
const AUTH_PATH: &str = "/auth";
/// What the Android sign-in expects its client to say it is.
const USER_AGENT: &str = "GoogleAuth/1.4";
/// The signature of the Android Keep app (a public constant of the app, not of the owner).
pub const KEEP_CLIENT_SIG: &str = "38918a453d07199354f8b19af05ec6562ced5788";
/// The app the access token is asked for as, and what it is asked for.
pub const KEEP_APP: &str = "com.google.android.keep";
pub const KEEP_SCOPES: &str = "oauth2:https://www.googleapis.com/auth/memento https://www.googleapis.com/auth/reminders";
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ANSWER: usize = 64 * 1024;
/// The longest secret taken: what a header or a form field can carry.
const MAX_SECRET: usize = 8 * 1024;

pub use mailcraft_model::keep::Secret;

/// One request to the Android sign-in server: the form fields, and the status and body of the
/// answer. [`exchange_token`] and [`access_token`] go to Google with it; a test hands in its own.
pub type AuthFetch<'a> = dyn Fn(&[(&str, &str)]) -> Result<(u16, Vec<u8>), NetError> + 'a;

/// Whether a master token may go to `url`: Google's Android sign-in server, over TLS, on its
/// usual port, and no other place.
pub(crate) fn may_go_to(url: &Url) -> Result<(), NetError> {
    let host = url.host.trim_end_matches('.');
    if url.secure && url.port == 0 && host.eq_ignore_ascii_case(AUTH_HOST) {
        return Ok(());
    }
    Err(NetError::Config("a Google Keep sign-in is only used with Google's own sign-in server".into()))
}

/// What the Keep sign-in calls this installation: a fixed-length name made from the Google
/// account's address, the same every time (the master token belongs to it, so the access tokens
/// are asked for as the same device). Nothing of the address can be read back from it.
pub fn android_id(email: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, format!("mailcraft-keep:{}", email.trim().to_ascii_lowercase()).as_bytes());
    digest.as_ref().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn usable(secret: &str) -> bool {
    !secret.is_empty() && secret.len() <= MAX_SECRET && secret.bytes().all(|b| b.is_ascii_graphic())
}

fn valid_email(email: &str) -> bool {
    let email = email.trim();
    !email.is_empty() && email.len() <= 254 && !email.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn refused(what: &str) -> NetError {
    NetError::Proto(ProtoError::Auth(what.to_string().into()))
}

/// The one thing Google's answer is read for: the value of `want` (`Token` or `Auth`), when it is
/// there. Whatever else it says is dropped; the answer's bytes are overwritten.
fn read_answer(status: u16, mut body: Vec<u8>, want: &str) -> Result<Secret, NetError> {
    let result = parse_answer(status, &body, want);
    body.zeroize();
    result
}

fn parse_answer(status: u16, body: &[u8], want: &str) -> Result<Secret, NetError> {
    if status == 429 || (500..=599).contains(&status) {
        return Err(NetError::Busy { text: "Google's sign-in is busy; try again later".into(), retry_after: None });
    }
    let text = String::from_utf8_lossy(body);
    let mut found: Option<Secret> = None;
    let mut code = String::new();
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else { continue };
        if name == want && found.is_none() && usable(value) {
            found = Some(Secret::new(value.to_string()));
        } else if name == "Error" && code.is_empty() {
            code = value.chars().take(40).collect();
        }
    }
    // (An answer that names an error is a refusal, whatever else it holds.)
    if status == 200
        && code.is_empty()
        && let Some(secret) = found
    {
        return Ok(secret);
    }
    // (Fixed sentences: what Google wrote is never repeated; it may carry a token.)
    Err(match code.as_str() {
        "NeedsBrowser" => refused("Google wants this sign-in to be made in a browser first: sign in again on the page the instructions name"),
        "BadAuthentication" => refused("Google didn't take the sign-in (the code may have expired, or the Keep sign-in was removed): connect again"),
        _ => refused("Google refused the Keep sign-in: connect again"),
    })
}

fn exchange_fields<'a>(email: &'a str, cookie: &'a str, android: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("accountType", "HOSTED_OR_GOOGLE"),
        ("Email", email),
        ("has_permission", "1"),
        ("add_account", "1"),
        ("ACCESS_TOKEN", "1"),
        ("Token", cookie),
        ("service", "ac2dm"),
        ("source", "android"),
        ("androidId", android),
        ("device_country", "us"),
        ("operatorCountry", "us"),
        ("lang", "en"),
        ("sdk_version", "17"),
        ("google_play_services_version", "240913000"),
        ("client_sig", KEEP_CLIENT_SIG),
        ("callerSig", KEEP_CLIENT_SIG),
        ("droidguard_results", "dummy123"),
    ]
}

fn oauth_fields<'a>(email: &'a str, master: &'a str, android: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("accountType", "HOSTED_OR_GOOGLE"),
        ("Email", email),
        ("has_permission", "1"),
        ("EncryptedPasswd", master),
        ("service", KEEP_SCOPES),
        ("source", "android"),
        ("androidId", android),
        ("app", KEEP_APP),
        ("client_sig", KEEP_CLIENT_SIG),
        ("device_country", "us"),
        ("operatorCountry", "us"),
        ("lang", "en"),
        ("sdk_version", "17"),
        ("google_play_services_version", "240913000"),
    ]
}

/// [`exchange_token`] through `fetch`. The cookie's buffer is overwritten before this returns,
/// whatever came of it.
pub fn exchange_token_with(fetch: &AuthFetch<'_>, email: &str, cookie: &mut String) -> Result<Secret, NetError> {
    let result = if !valid_email(email) {
        Err(refused("that is not a Google account's address"))
    } else if !usable(cookie) {
        Err(refused("the code pasted is not an oauth_token cookie"))
    } else {
        let android = android_id(email);
        let fields = exchange_fields(email.trim(), cookie, &android);
        fetch(&fields).and_then(|(status, body)| read_answer(status, body, "Token"))
    };
    cookie.zeroize();
    result
}

/// Trade the `oauth_token` cookie of Google's EmbeddedSetup page for the account's master token.
/// The cookie is overwritten.
pub fn exchange_token(email: &str, cookie: &mut String) -> Result<Secret, NetError> {
    exchange_token_with(&client(), email, cookie)
}

/// [`access_token`] through `fetch`.
pub fn access_token_with(fetch: &AuthFetch<'_>, email: &str, master: &Secret) -> Result<Secret, NetError> {
    if !valid_email(email) {
        return Err(refused("that is not a Google account's address"));
    }
    if !usable(master.expose()) {
        return Err(refused("there is no Google Keep sign-in"));
    }
    let android = android_id(email);
    let fields = oauth_fields(email.trim(), master.expose(), &android);
    fetch(&fields).and_then(|(status, body)| read_answer(status, body, "Auth"))
}

/// Trade the master token for an access token for Google Keep (about an hour), kept in memory.
pub fn access_token(email: &str, master: &Secret) -> Result<Secret, NetError> {
    access_token_with(&client(), email, master)
}

/// The requests to Google's Android sign-in server.
fn client() -> impl Fn(&[(&str, &str)]) -> Result<(u16, Vec<u8>), NetError> {
    move |fields: &[(&str, &str)]| {
        let mut url = Url::parse(AUTH)?;
        may_go_to(&url)?;
        url.path = AUTH_PATH.to_string();
        let mut body = http::form(fields);
        let headers = [("User-Agent", USER_AGENT), ("Content-Type", "application/x-www-form-urlencoded"), ("Accept-Encoding", "identity")];
        let request = Request { method: "POST", headers: &headers, body: Some(body.as_bytes()), patience: TIMEOUT, max_body: MAX_ANSWER };
        let reply = http::request(&url, &request);
        body.zeroize();
        // (A transport error says where it failed, never what was sent.)
        reply.map(|reply| (reply.status, reply.body))
    }
}
