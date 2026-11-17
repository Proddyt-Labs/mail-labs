// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Google Keep jobs. One trades the `oauth_token` cookie the owner pasted
//! for the account's master token ([`connect`]); the other keeps an account's notes in step with
//! Keep ([`run`]): an access token from the master token, what waits to go up told to Keep and
//! what changed there read, in one run.
//!
//! **Secrets.** The cookie, the master token and the access token are [`Secret`]s here too: no
//! job, result or error of this module shows one, in `Debug`, in an error line or in a log. An
//! error line is a fixed sentence of the network layer, scrubbed once more here of any secret
//! the job holds. The master token comes in with the job and goes back out only as the result of
//! a [`connect`], to be put in the system's credential store by the window; nothing here writes it.
//!
//! What to run and when is the engine's ([`mailcraft_engine::sync_driver`]); what the answer
//! means for the mailbox is [`mailcraft_engine::cmd::keep`], applied by the front end.

use std::fmt;

use mailcraft_model::keep::{KeepDownload, KeepUpload, Secret};
use mailcraft_model::{Account, Id};
#[cfg(not(target_arch = "wasm32"))]
use mailcraft_net::NetError;

/// What the host is asked to do for one account's notes.
pub struct KeepJob {
    pub account: Account,
    /// The account's master token, read from the system's credential store (or kept in memory by
    /// the window for this session): the job trades it for an access token of its own.
    pub master: Secret,
    /// What Keep is to be told first.
    pub upload: KeepUpload,
}

impl fmt::Debug for KeepJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeepJob").field("account", &self.account.id).field("master", &self.master).field("nodes", &self.upload.nodes.len()).finish()
    }
}

/// What a [`KeepJob`] came to. Never an error as a whole: every trouble is a line in the answer.
#[derive(Debug, Default)]
pub struct KeepResult {
    pub account: Id,
    /// Keep no longer takes the sign-in: the owner is to connect again.
    pub sign_in_refused: bool,
    /// Keep says this account may not use it this way.
    pub denied: bool,
    pub download: Option<KeepDownload>,
    pub errors: Vec<String>,
}

/// What the host is asked to do with a pasted cookie.
pub struct KeepConnectJob {
    pub account: Account,
    pub cookie: Secret,
}

impl fmt::Debug for KeepConnectJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeepConnectJob").field("account", &self.account.id).field("cookie", &self.cookie).finish()
    }
}

/// What a [`KeepConnectJob`] came to: the master token, or why not (a fixed sentence).
#[derive(Debug, Default)]
pub struct KeepConnectResult {
    pub account: Id,
    pub master: Option<Secret>,
    pub error: Option<String>,
}

/// `text` with every one of `secrets` blanked out, should an error have written one back.
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    secrets.iter().filter(|s| s.len() >= 6).fold(text.to_string(), |text, secret| text.replace(secret, "[hidden]"))
}

// The browser can't open sockets: `mailcraft_net` is empty there, so only the jobs' data (above) builds.

/// Trade the cookie for the master token.
#[cfg(not(target_arch = "wasm32"))]
pub fn connect(job: KeepConnectJob) -> KeepConnectResult {
    connect_with(job, &mailcraft_net::keep_auth::exchange_token)
}

/// [`connect`], with how the cookie is traded as `exchange` (the tests' own, which asks nobody).
#[cfg(not(target_arch = "wasm32"))]
pub fn connect_with(job: KeepConnectJob, exchange: &dyn Fn(&str, &mut String) -> Result<Secret, NetError>) -> KeepConnectResult {
    let mut out = KeepConnectResult { account: job.account.id, ..Default::default() };
    // (The buffer the exchange overwrites is a copy: the job's own secret is overwritten when it drops.)
    let mut cookie = job.cookie.expose().to_string();
    match exchange(job.account.email.trim(), &mut cookie) {
        Ok(master) => out.master = Some(master),
        Err(e) => {
            let mut said = e.to_string();
            said = scrub(&said, &[job.cookie.expose()]);
            out.error = Some(said);
        }
    }
    out
}

/// Run the job: an access token from the master token, what waits to go told to Keep, and what
/// changed there read.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(job: KeepJob) -> KeepResult {
    run_with(job, &mailcraft_net::keep_auth::access_token, &mailcraft_net::google_keep::sync)
}

/// [`run`], with how the access token is had (`renew`) and how Keep is spoken to (`sync`) handed
/// in: the tests' own ask nobody.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_with(
    job: KeepJob,
    renew: &dyn Fn(&str, &Secret) -> Result<Secret, NetError>,
    sync: &dyn Fn(&Secret, &KeepUpload) -> Result<KeepDownload, NetError>,
) -> KeepResult {
    let mut out = KeepResult { account: job.account.id, ..Default::default() };
    let email = job.account.email.trim().to_string();
    let access = match renew(&email, &job.master) {
        Ok(access) => access,
        Err(e) if e.is_refused_sign_in() => {
            out.sign_in_refused = true;
            return out;
        }
        Err(e) => {
            out.errors.push(format!("Couldn't sign in to Google Keep: {}", scrub(&e.to_string(), &[job.master.expose()])));
            return out;
        }
    };
    match sync(&access, &job.upload) {
        Ok(download) => out.download = Some(download.for_account(&job.account)),
        Err(NetError::Forbidden(_)) => out.denied = true,
        Err(e) if e.is_refused_sign_in() => out.sign_in_refused = true,
        Err(e) => {
            let said = scrub(&e.to_string(), &[job.master.expose(), access.expose()]);
            out.errors.push(format!("Couldn't read Google Keep: {said}"));
        }
    }
    out
}
