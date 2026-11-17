// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the notes of the owner's Google Keep, kept in step in the window, for an
//! account the owner connected (File › Info › Connect Google Keep, or the Notes module).
//!
//! **Connecting.** Google has no public sign-in for Keep. The owner signs in on Google's own page
//! (`accounts.google.com/EmbeddedSetup`), copies the `oauth_token` cookie that page sets and pastes
//! it into a masked box here ([`Dialog::KeepConnect`]). A background job trades it for the
//! account's master token ([`mailcraft_host::keep::connect`]); the window puts that in the system's
//! credential store ([`KEEP_SECRET`], under the account's own name) and links the account
//! (`keep.link`). **The cookie is typed into the box and nowhere else**: it is never a command's
//! parameter (so no agent, script or control channel can give or read one), it is moved out of the
//! box into a [`Secret`] that is overwritten when dropped, and the box is cleared.
//!
//! **The master token** is in the credential store and, for this session, in memory
//! ([`MailApp::gkeep_master`], never serialised; its `Debug` is redacted). It is never in a
//! setting, the mailbox, a log, a toast, an error line, a problem report or a control answer.
//! When the owner turned off remembering sign-ins, it is kept in memory alone and the owner
//! connects again at the next start.
//!
//! **Syncing.** A job of its own, as the tasks' ([`crate::task_sync`]): when the notes are due
//! (every few minutes, a moment after a change here) the window hands the host a [`KeepJob`]
//! with what is to be told to Keep (`sync.keepChanges`); the host trades the master token for an
//! access token (about an hour; in memory for that run alone), tells Keep, reads what changed
//! there, and the window takes it in (`sync.applyKeep`, [`mailcraft_engine::cmd::keep`]).
//!
//! **Disconnecting** deletes the credential and the in-memory copy, and the notes stay on this
//! computer. What it can't do is take the access away at Google: that is removing this device in
//! Google Account › Security › Your devices (or changing the password), which the window says.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::keep as keep_cmd;
use mailcraft_engine::sync_driver::{JobBook, Pacing, Start, fingerprint};
use mailcraft_model::Id;
use mailcraft_model::keep::Secret;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, now_ms};

pub use mailcraft_host::keep::{KeepConnectJob, KeepConnectResult, KeepJob, KeepResult};

/// What a Google Keep master token is kept as in the system's credential store (with the
/// account's own name, [`MailApp::secret_key`]): never a mail sign-in's.
pub(crate) const KEEP_SECRET: &str = "googleKeep";

/// Where Google's own sign-in page is, for the owner to sign in on and copy the cookie from.
pub(crate) const SETUP_PAGE: &str = "https://accounts.google.com/EmbeddedSetup";

/// What a job found of an account's Keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeepStatus {
    /// A job read it.
    Given,
    /// Google no longer takes the sign-in: the owner is to connect again.
    SignInOver,
    /// Google says this account may not use Keep this way.
    Refused,
}

/// The state of the Keep jobs, in the window.
#[derive(Default)]
pub struct KeepSync {
    /// The job under way: for which account, since when, the [`fingerprint`] of the master
    /// token it was given, the mailbox opening it was started for, and the answer to come.
    rx: Option<(Id, f64, u64, u64, Receiver<KeepResult>)>,
    /// The trade of a pasted cookie under way: for which account, and the answer to come.
    connecting: Option<(Id, u64, Receiver<KeepConnectResult>)>,
    book: JobBook,
    pub(crate) status: HashMap<Id, KeepStatus>,
    /// How many changes wait per account, worked out for the mailbox as of this revision.
    ready: Option<(u64, HashMap<Id, usize>)>,
}

impl KeepSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.rx.is_some() || self.connecting.is_some()
    }
}

impl MailApp {
    /// The account's master token: the one in memory, else what the system's credential store
    /// gave the background read. (Never put in a log or shown: a [`Secret`].)
    pub(crate) fn gkeep_master(&self, account: Id) -> Option<Secret> {
        let kept = self.gkeep_master.get(&account).filter(|m| !m.is_empty()).cloned();
        kept.or_else(|| self.remembered(KEEP_SECRET, account).filter(|m| !m.is_empty()).map(Secret::new))
    }

    /// Look whether any account's notes are due, and start a job when they are. Called every
    /// frame. (Public for a host's tests, which have no window.)
    pub fn gkeep_tick_at(&mut self, now: f64) -> bool {
        if !self.auto_sync || self.services.keep_sync.is_none() || self.gkeep.rx.is_some() || self.gkeep.connecting.is_some() {
            return false;
        }
        // One thing at a time: the calendar's job, a Send/Receive and a sign-in go first.
        if self.job_busy().blocks(Start::Tasks) {
            return false;
        }
        for account in keep_cmd::keep_accounts(&self.session.mailbox) {
            if matches!(self.gkeep.status.get(&account), Some(KeepStatus::SignInOver | KeepStatus::Refused)) || self.gkeep_master(account).is_none() {
                continue;
            }
            let rev = self.session.rev();
            if self.gkeep.ready.as_ref().is_none_or(|(at, _)| *at != rev) {
                self.gkeep.ready = Some((rev, HashMap::new()));
            }
            let ready = match self.gkeep.ready.as_ref().and_then(|(_, m)| m.get(&account).copied()) {
                Some(n) => n,
                None => {
                    let n = keep_cmd::keep_waiting(&self.session.mailbox, account);
                    if let Some((_, m)) = &mut self.gkeep.ready {
                        m.insert(account, n);
                    }
                    n
                }
            };
            if !self.gkeep.book.due(&Pacing::TASKS, account, ready, now) {
                continue;
            }
            if self.start_gkeep_job(account, now) {
                return true;
            }
        }
        false
    }

    pub(crate) fn gkeep_tick(&mut self, ctx: &egui::Context) {
        self.poll_gkeep_connect();
        self.gkeep_tick_at(now_ms());
        if self.gkeep.rx.is_some() || self.gkeep.connecting.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.services.keep_sync.is_some() && self.auto_sync && !keep_cmd::keep_accounts(&self.session.mailbox).is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }

    fn start_gkeep_job(&mut self, account: Id, now: f64) -> bool {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        let Some(master) = self.gkeep_master(account) else { return false };
        let Ok(upload) = keep_cmd::keep_upload(&self.session.mailbox, account) else { return false };
        let given = fingerprint(master.expose());
        let job = KeepJob { account: owner, master, upload };
        let Some(run) = self.services.keep_sync.as_ref().filter(|_| !self.offline()) else { return false };
        let rx = run(job);
        self.gkeep.rx = Some((account, now, given, self.session.openings(), rx));
        self.gkeep.book.started(account, now);
        true
    }

    /// The job answered: take in what Keep made of the changes, and what it has. (Public for a
    /// host's tests.)
    pub fn poll_gkeep(&mut self) {
        let Some((account, started, _given, opening, rx)) = &self.gkeep.rx else { return };
        let (account, started) = (*account, *started);
        // For a mailbox that has been closed since: its ids may name other accounts and notes in
        // this one, and the answer is nobody's.
        if *opening != self.session.openings() {
            log::info!("keep: the answer is for a mailbox that is no longer open; let go of");
            self.gkeep = KeepSync::default();
            return;
        }
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("keep, account {account}: the job ended without an answer (its thread ended early)");
                KeepResult {
                    account,
                    errors: vec![tl!("The notes couldn't be kept in step: the job stopped unexpectedly").to_string()],
                    ..Default::default()
                }
            }
        };
        self.gkeep.rx = None;
        let mut errors = result.errors;
        if result.sign_in_refused {
            log::warn!("keep: Google no longer takes the sign-in; waiting for a new one");
            self.gkeep.status.insert(account, KeepStatus::SignInOver);
        } else if result.denied {
            self.gkeep.status.insert(account, KeepStatus::Refused);
        } else if result.download.is_some() {
            self.gkeep.status.insert(account, KeepStatus::Given);
        }
        let mut worked = false;
        if let Some(download) = &result.download {
            // (Taken in for the account as the job had it: `for_account` was applied by the job.)
            match keep_cmd::apply_keep(&mut self.session, download) {
                Ok(merged) => {
                    errors.extend(merged.messages);
                    worked = true;
                    // A forced read from the start: the next job asks with no version.
                    if download.resync {
                        self.gkeep.book.forget_checked(account);
                    }
                }
                Err(e) => errors.push(crate::i18n::fmt(tl!("The notes couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        if worked && errors.is_empty() {
            self.gkeep.book.worked(account);
        } else if !result.sign_in_refused && !result.denied {
            self.gkeep.book.failed(account, started);
        }
        self.note_problems_from("keep", account, &errors, None, false);
    }

    // ---------------------------------------------------------------- connecting

    /// Whether File › Info and the Notes module offer to connect an account to Google Keep: an
    /// account of the owner's own (not a shared mailbox) at Google (Gmail's servers, a Google
    /// sign-in, or a Gmail address) that isn't connected yet, in a build that can sync.
    pub(crate) fn offers_google_keep(&self, account: Id) -> bool {
        let Some(a) = self.session.mailbox.account(account) else { return false };
        let at_google = a.is_gmail()
            || a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Google)
            || a.email.trim().rsplit_once('@').is_some_and(|(_, domain)| mailcraft_model::gmail::is_gmail_host(domain));
        self.services.keep_connect.is_some() && !a.is_shared() && !a.keeps_google_keep() && a.kind.has_server() && at_google
    }

    /// The first account the Notes module can offer Google Keep to.
    pub(crate) fn gkeep_offer_account(&self) -> Option<Id> {
        self.session.mailbox.accounts_in_order().into_iter().map(|a| a.id).find(|id| self.offers_google_keep(*id))
    }

    /// What File › Info says of Google Keep for an account, and whether it can be connected (or
    /// connected again) from here; `None` for an account that has nothing to say about it.
    pub(crate) fn gkeep_line(&self, account: Id) -> Option<(String, bool)> {
        let a = self.session.mailbox.account(account)?;
        if a.keeps_google_keep() {
            let waiting = keep_cmd::keep_waiting(&self.session.mailbox, account);
            return Some(match self.gkeep.status.get(&account) {
                _ if self.gkeep_master(account).is_none() => (tl!("Notes: Google Keep needs you to connect again").to_string(), true),
                Some(KeepStatus::Given) if waiting == 0 => (tl!("Notes: kept in step with Google Keep").to_string(), false),
                Some(KeepStatus::Given) => (tl!("Notes: Google Keep, changes waiting to be sent").to_string(), false),
                Some(KeepStatus::SignInOver) => (tl!("Notes: Google Keep no longer takes the sign-in; connect again").to_string(), true),
                Some(KeepStatus::Refused) => (tl!("Notes: Google Keep can't be used for this account").to_string(), true),
                None => (tl!("Notes: Google Keep, not read yet").to_string(), false),
            });
        }
        self.offers_google_keep(account).then(|| (tl!("Notes: Google Keep not connected").to_string(), true))
    }

    /// Open the box where the owner pastes the cookie of Google's sign-in page. Nothing is sent
    /// before the owner submits it.
    pub(crate) fn ask_google_keep(&mut self, account: Id) -> bool {
        if self.dialog.is_some() || self.session.mailbox.account(account).is_none_or(|a| a.is_shared()) || self.services.keep_connect.is_none() {
            return false;
        }
        self.dialog = Some(Dialog::KeepConnect { account, cookie: String::new(), shown: false });
        true
    }

    /// The box was submitted: trade the cookie. The cookie leaves the box here (the box is
    /// cleared) and is a [`Secret`] from now on.
    pub(crate) fn begin_keep_connect(&mut self, account: Id, cookie: &mut String) -> Result<(), String> {
        let owner = self.session.mailbox.account(account).filter(|a| !a.is_shared()).cloned();
        let Some(owner) = owner else {
            zeroize::Zeroize::zeroize(cookie);
            return Err(tl!("This account's notes can't be kept in step with Google Keep").to_string());
        };
        if self.gkeep.connecting.is_some() || self.services.keep_connect.is_none() {
            zeroize::Zeroize::zeroize(cookie);
            return Err(tl!("Google Keep can't be connected now").to_string());
        }
        let secret = Secret::new(std::mem::take(cookie));
        // (What is left in the box's buffer is overwritten.)
        zeroize::Zeroize::zeroize(cookie);
        let Some(connect) = self.services.keep_connect.as_ref().filter(|_| !self.offline()) else {
            return Err(tl!("Google Keep can't be connected while MailCraft is working offline").to_string());
        };
        let rx = connect(KeepConnectJob { account: owner, cookie: secret });
        self.gkeep.connecting = Some((account, self.session.openings(), rx));
        Ok(())
    }

    /// The trade of the cookie answered: with a master token it is kept (the system's credential
    /// store and this session's memory) and the account is linked; without, why not is said.
    pub(crate) fn poll_gkeep_connect(&mut self) {
        let Some((account, opening, rx)) = &self.gkeep.connecting else { return };
        let (account, opening) = (*account, *opening);
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => KeepConnectResult {
                account,
                error: Some(tl!("Google Keep wasn't connected: the job stopped unexpectedly").to_string()),
                ..Default::default()
            },
        };
        self.gkeep.connecting = None;
        if opening != self.session.openings() {
            return;
        }
        match answer.master {
            Some(master) if !master.is_empty() => {
                // (Linked first: a master token of an account that isn't linked is not kept.)
                if let Err(e) = self.run("keep.link", json!({"account": account, "linked": true})) {
                    self.status(crate::i18n::fmt(tl!("Google Keep wasn't connected: {error}"), &[("error", &e)]));
                    return;
                }
                self.remember(KEEP_SECRET, account, master.expose());
                self.gkeep_master.insert(account, master);
                log::info!("keep, account {account}: Google Keep connected; the next job reads it");
                self.gkeep.status.remove(&account);
                self.gkeep.book.worked(account);
                self.gkeep.book.forget_checked(account);
                self.status(tl!("Google Keep connected: your notes are kept in step with it"));
            }
            _ => {
                // (A fixed sentence of the network layer: never what Google wrote.)
                let said = answer.error.unwrap_or_else(|| tl!("Google didn't give a Keep sign-in").to_string());
                self.status(crate::i18n::fmt(tl!("Google Keep wasn't connected: {error}"), &[("error", &said)]));
            }
        }
    }

    /// File › Info's Disconnect: the account's notes stay on this computer, Google is told
    /// nothing, the credential is deleted and so is the copy in memory. The owner is told what
    /// only Google can do (removing the device).
    pub(crate) fn disconnect_google_keep(&mut self, account: Id) {
        let _ = self.run("keep.link", json!({"account": account, "linked": false}));
        self.gkeep_master.remove(&account);
        self.gkeep.status.remove(&account);
        if let Some(a) = self.session.mailbox.account(account).cloned() {
            self.forget_account_secret(KEEP_SECRET, &a);
        }
        self.dialog = Some(Dialog::Report {
            name: "keepDisconnected",
            title: tl!("Google Keep disconnected").to_string(),
            lines: vec![tl!(
                "The notes stay on this computer. MailCraft has forgotten the sign-in, but that doesn't take the access away at Google: to do that, remove this device in your Google Account, under Security › Your devices, or change your password."
            )
            .to_string()],
        });
    }

    /// `ui.connectGoogleKeep`: opens the box for the account. It takes no sign-in: the cookie is
    /// typed into the box by the owner.
    pub(crate) fn begin_google_keep(&mut self, account: Id) -> Result<serde_json::Value, String> {
        if !self.offers_google_keep(account) && !self.session.mailbox.account(account).is_some_and(|a| a.keeps_google_keep()) {
            return Err(tl!("This account's notes can't be kept in step with Google Keep").to_string());
        }
        if !self.ask_google_keep(account) {
            return Err(tl!("Google Keep can't be connected now").to_string());
        }
        Ok(json!({"pending": "keepConnect", "account": account}))
    }
}

#[cfg(test)]
impl KeepSync {
    /// For a test: whether the connect job is under way.
    pub(crate) fn connecting(&self) -> bool {
        self.connecting.is_some()
    }
}
