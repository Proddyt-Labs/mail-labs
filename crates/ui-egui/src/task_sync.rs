// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks kept in step with Microsoft To Do in the window (#248), for the accounts that sign in
//! with Microsoft.
//!
//! **Local to-dos.** Those that exist only on this computer are never sent unasked: when an
//! account's To Do works and there are such to-dos, the window asks once ([`MailApp::offer_local_tasks`]);
//! Not now keeps them here, and File › Info offers it again.
//!
//! A job of its own, like the calendar's: when the to-dos are due (every few minutes, a moment
//! after a change here) the window hands the host a [`TasksJob`] with what is to be told to the
//! server (`sync.taskChanges`); the host tells it, then reads the lists and tasks whole, and the
//! window takes in what the server made of the changes (`sync.applyTaskChanges`) and then what it
//! has (`sync.applyTasks`). The merge is the engine's ([`mailcraft_engine::cmd::tasks_remote`]).
//!
<<<<<<< HEAD
//! **The permission.** Reading and changing the lists takes `Tasks.ReadWrite`. No sign-in page
//! asks for it (a mail sign-in must never wait on it, a tenant may need an administrator for it): the
//! first job finds its sign-in doesn't cover it ([`TasksResult::consent_needed`]), nothing of the
//! to-dos is read or sent, mail and calendar go on as ever, and the window asks the owner, by
//! itself, with a question first and then Microsoft's page ([`MailApp::ask_tasks`]). File › Info
//! says where it stands, with a way to ask again. A no that is not the owner's to change is said
//! as for the Favorites ([`crate::favorites_sync::NEEDS_APPROVAL`]…).
//!
//! **Google Tasks** (Proddyt Labs fork). An account whose to-dos Microsoft doesn't keep (a Gmail
//! account with an app password, say) can have them kept in step with Google Tasks instead: the
//! owner chooses Connect Google Tasks in File › Info ([`MailApp::ask_google_tasks`]), Google's page
//! asks for the Tasks permission alone (`ui.connectGoogleTasks`), and with a yes the account is
//! linked (`tasks.linkGoogle`) and its refresh token for the tasks kept apart from any mail
//! sign-in ([`GOOGLE_TASKS_SECRET`]). The same job then runs against Google (`TasksJob::oauth`
//! names Google), and the same merge takes in what it brings. A build without a Google
//! registration can't connect, and File › Info says so.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
=======
//! **The permission.** Reading and changing the lists takes `Tasks.ReadWrite`. A new sign-in's
//! page asks for it beside the rest, as an extra a page that won't have it is asked again without
//! (#661). For an account that signed in before, the first job finds its sign-in doesn't cover it
//! ([`TasksResult::consent_needed`]), nothing of the to-dos is read or sent, mail and calendar go
//! on as ever, and the window asks the owner the one question of [`crate::server_sync`], then
//! Microsoft's page, for the to-dos and the mailbox's settings together. File › Info says where it
//! stands, with Ask again. A no that is not the owner's to change is said as for the Favorites
//! ([`crate::favorites_sync::NEEDS_APPROVAL`]…).
>>>>>>> upstream/main

use mailcraft_engine::cmd::tasks_remote;
use mailcraft_engine::sync_driver::{TasksFacts, TasksQuestion};
use mailcraft_host::tasks_job::TasksJobs;
use mailcraft_model::{Id, LocalTasks};
use serde_json::json;

use crate::dialogs::{Dialog, Question};
use crate::{MailApp, now_ms};

pub(crate) use mailcraft_engine::sync_driver::{Permission, fingerprint};
pub use mailcraft_host::tasks::{TasksJob, TasksResult};

<<<<<<< HEAD
/// Proddyt Labs fork: what a Google Tasks refresh token is kept as in the system's credential
/// store (with the account's own name, [`MailApp::secret_key`]): never the mail sign-in's.
pub(crate) const GOOGLE_TASKS_SECRET: &str = "googleTasks";

/// What a to-do job was started for: the mailbox opening ([`mailcraft_engine::Session::openings`])
/// and the account as it was, whose address the answer is taken in for (AU3-08).
struct Begun {
    opening: u64,
    account: mailcraft_model::Account,
}

=======
>>>>>>> upstream/main
/// The state of the to-do jobs, in the window.
#[derive(Default)]
pub struct TaskSync {
    /// The job under way, kept by the host's driver of it ([`TasksJobs`]).
    jobs: TasksJobs,
    /// The question about the to-dos that exist only here is on screen.
    pub(crate) offer_open: bool,
}

impl TaskSync {
    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.jobs.busy()
    }
}

impl MailApp {
<<<<<<< HEAD
    /// Whether the to-dos of `account` wait for the owner's permission, or for a new sign-in.
    fn tasks_blocked(&self, account: Id) -> bool {
        let refresh = if tasks_remote::keeps_google_tasks(&self.session.mailbox, account) {
            self.google_tasks_refresh.get(&account).map(String::as_str)
        } else {
            self.tokens.get(&account).map(|t| t.refresh.as_str())
        };
        Permission::blocks(self.tasks.perm.get(&account), refresh)
    }

    /// The refresh token a to-do job of `account` renews its token with: for Google Tasks its own
    /// (Proddyt Labs fork), else the account's sign-in.
    fn tasks_refresh(&self, account: Id) -> Option<String> {
        if tasks_remote::keeps_google_tasks(&self.session.mailbox, account) {
            let kept = self.google_tasks_refresh.get(&account).cloned().filter(|r| !r.is_empty());
            return kept.or_else(|| self.remembered(GOOGLE_TASKS_SECRET, account)).filter(|r| !r.is_empty());
        }
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        kept.or_else(|| self.remembered("refresh", account)).filter(|r| !r.is_empty())
    }

=======
>>>>>>> upstream/main
    /// Look whether any account's to-dos are due, and start a job when they are. Called every
    /// frame. (Public for a host's tests, which have no window.) What is due is the engine's to
    /// say ([`mailcraft_engine::Session::tasks_due`]); this starts the job on the host.
    pub fn tasks_tick_at(&mut self, now: f64) -> bool {
        self.session.driver.tasks.clock = now;
        self.tasks_ask_tick();
        let facts = TasksFacts { reachable: self.services.graph_tasks.is_some(), running: self.tasks.busy(), busy: self.job_busy() };
        let tokens = &self.tokens;
        let due = self.session.tasks_due(&facts, |account| tokens.get(&account).map(|t| t.refresh.clone()), now);
        for account in due {
            if self.start_tasks_job(account, now) {
                return true;
            }
        }
        false
    }

    pub(crate) fn tasks_tick(&mut self, ctx: &egui::Context) {
        self.tasks_tick_at(now_ms());
        if self.tasks.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else if self.services.graph_tasks.is_some()
            && self.session.driver.auto_sync()
            && !tasks_remote::task_accounts(&self.session.mailbox).is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_secs(5));
        }
    }

    fn start_tasks_job(&mut self, account: Id, now: f64) -> bool {
<<<<<<< HEAD
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return false };
        // (Google Tasks: Google's registration and the account's own sign-in for the tasks.)
        let (oauth, google_client_secret) = if owner.keeps_google_tasks() {
            if !self.google_registration_ready() {
                return false;
            }
            let google = mailcraft_model::OAuth { provider: mailcraft_model::OAuthProvider::Google, ..Default::default() };
            (self.effective_oauth(google), self.google_client_secret().to_string())
        } else {
            let Some(oauth) = owner.oauth.clone() else { return false };
            (self.effective_oauth(oauth), String::new())
        };
        let Some(refresh) = self.tasks_refresh(account) else {
            // (Nothing to renew with: Send/Receive signs in first, or Google Tasks is to be connected.)
            return false;
        };
        let Ok(upload) = tasks_remote::task_upload(&self.session.mailbox, account) else { return false };
        let given = fingerprint(&refresh);
        let deletes = tasks_remote::carries_delete(&upload);
        let zone = self.session.zone();
        let begun = Begun { opening: self.session.openings(), account: owner.clone() };
        let job = TasksJob { account: owner, oauth, refresh, upload, zone, cancel: Default::default(), google_client_secret };
        let Some(graph) = self.services.graph_tasks.as_ref().filter(|_| !self.offline()) else { return false };
        let rx = graph(job);
        // A delete that goes out can't be undone: the task would come back here, gone there (#706).
        tasks_remote::mark_sent(&mut self.session, deletes);
        self.tasks.rx = Some((account, now, given, begun, rx));
        self.tasks.book.started(account, now);
        true
=======
        let Some(owner) = self.session.mailbox.account(account).filter(|a| a.oauth.is_some()).map(|a| a.id) else { return false };
        let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", owner));
        let offline = self.offline();
        let ids = mailcraft_engine::sync_driver::ClientIds {
            microsoft_chosen: &self.ui.microsoft_client_id,
            microsoft_built_in: &self.services.microsoft_client_id,
            google_chosen: &self.ui.google_client_id,
            google_built_in: &self.services.google_client_id,
        };
        let graph = self.services.graph_tasks.as_ref().filter(|_| !offline);
        // What the job is handed, the delete marked as gone and the engine told are the host's.
        let oauth = |oauth| mailcraft_engine::sync_driver::effective_oauth(oauth, &ids);
        self.tasks.jobs.start(&mut self.session, account, |_| refresh, oauth, now, |job| graph.map(|graph| graph(job)))
>>>>>>> upstream/main
    }

    /// The job answered: keep the refresh token, take in what the server made of the changes, and
    /// what it has. (Public for a host's tests.)
    pub fn poll_tasks(&mut self) {
        // For a mailbox that has been closed since: its ids may name other accounts and tasks in
        // this one, and the answer is nobody's (AU3-08).
        if self.tasks.jobs.stale(&self.session) {
            log::info!("tasks: the answer is for a mailbox that is no longer open; let go of");
            self.tasks = TaskSync::default();
            return;
        }
<<<<<<< HEAD
        let ran_for = begun.account.clone();
        let google = ran_for.keeps_google_tasks();
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::warn!("tasks, account {account}: the job ended without an answer (its thread ended early)");
                TasksResult {
                    account,
                    errors: vec![tl!("The to-dos couldn't be kept in step: the job stopped unexpectedly").to_string()],
                    ..Default::default()
                }
            }
        };
        self.tasks.rx = None;
        let mut errors = result.errors;
        if let Some(tokens) = &result.tokens
=======
        let lang = crate::i18n::current();
        let Some(answer) = self.tasks.jobs.poll(&mut self.session, lang) else { return };
        let account = answer.account;
        let mut errors = answer.errors;
        if answer.stopped_early {
            errors.insert(0, tl!("The to-dos couldn't be kept in step: the job stopped unexpectedly").to_string());
        }
        // Somebody else's sign-in: the host refused the answer whole (nothing of it is taken in and
        // its token is not kept); the owner is told who to sign in as, as for any sign-in (#103).
        if let Some(foreign) = &answer.foreign {
            errors.push(self.say_signed_in_as_other(account, &foreign.signed_in_as));
        }
        if let Some(tokens) = &answer.tokens
>>>>>>> upstream/main
            && !tokens.refresh.is_empty()
            && !self.signed_in_as_other(account, tokens)
        {
            if google {
                // (Google Tasks' own sign-in: never put in mail's place.)
                self.remember(GOOGLE_TASKS_SECRET, account, &tokens.refresh);
                self.google_tasks_refresh.insert(account, tokens.refresh.clone());
            } else {
                self.remember("refresh", account, &tokens.refresh);
                if let Some(mail) = self.tokens.get_mut(&account) {
                    mail.refresh = tokens.refresh.clone();
                }
            }
        }
<<<<<<< HEAD
        let settling = self.tasks.settling.get(&account).copied().unwrap_or(0);
        if !result.consent_needed {
            self.tasks.settling.remove(&account);
        }
        if result.consent_needed && settling > 0 {
            log::info!("tasks, account {account}: refused just after the owner allowed them; trying again ({settling} more)");
            self.tasks.settling.insert(account, settling - 1);
            self.tasks.book.failed(account, started);
            self.tasks.book.forget_checked(account);
        } else if result.consent_needed {
            log::info!("tasks, account {account}: the sign-in doesn't cover to-dos; the owner is to be asked");
            if !matches!(self.tasks.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.tasks.perm.insert(account, Permission::Needed);
            }
        } else if result.denied {
            let said = if google {
                tl!("This account may not use Google Tasks").to_string()
            } else {
                tl!("This account may not use its to-dos through Microsoft Graph").to_string()
            };
            self.tasks.perm.insert(account, Permission::Refused(said));
        } else if result.sign_in_refused && result.tokens.is_none() && settling == 0 {
            // The refresh itself was refused, for mail too: never "not asked yet" with no way back
            // (#520). (Not when the owner allowed the to-dos since the job began: that sign-in is
            // newer than this answer.)
            log::warn!("tasks: the sign-in was refused; waiting for a new one");
            if !matches!(self.tasks.perm.get(&account), Some(Permission::Asking | Permission::Declined | Permission::Refused(_))) {
                self.tasks.perm.insert(account, Permission::SignInOver(given));
            }
        } else if result.sign_in_refused {
            // Graph refused a token just renewed: the next look tries again.
            log::warn!("tasks: the server refused the sign-in; trying again later");
        } else if result.download.is_some() || result.uploaded.is_some() {
            self.tasks.perm.insert(account, Permission::Given);
        }
        let mut worked = false;
        if let Some(uploaded) = &result.uploaded {
            // (Taken in for the account as the job had it, never relabelled with what is here now.)
            let uploaded = uploaded.clone().for_account(&ran_for);
            match tasks_remote::apply_task_changes(&mut self.session, &uploaded) {
                Ok(merged) => {
                    errors.extend(merged.messages);
                    worked = true;
                }
                Err(e) => errors.push(crate::i18n::fmt(tl!("The to-dos sent couldn't be noted: {error}"), &[("error", &e.to_string())])),
            }
        }
        if let Some(download) = &result.download {
            match tasks_remote::apply_tasks(&mut self.session, download) {
                Ok(_) => worked = true,
                Err(e) => errors.push(crate::i18n::fmt(tl!("The to-dos couldn't be taken in: {error}"), &[("error", &e.to_string())])),
            }
        }
        if worked && errors.is_empty() {
            self.tasks.book.worked(account);
        } else if !result.consent_needed && !result.sign_in_refused {
            self.tasks.book.failed(account, started);
        }
=======
>>>>>>> upstream/main
        self.note_problems_from("tasks", account, &errors, None, false);
    }

    /// Ask the owner, once, when a job found the sign-in doesn't cover to-dos: a question first,
    /// then Microsoft's page. Not while another dialog is up or File is open.
    fn tasks_ask_tick(&mut self) {
        // The question about the local to-dos went away unanswered: that is a "Not now", noted,
        // so it is not asked on every look (File › Info offers it again).
        if self.tasks.offer_open && !matches!(&self.dialog, Some(Dialog::Confirm { command, .. }) if command == "tasks.sendLocal") {
            self.tasks.offer_open = false;
            if self.session.mailbox.local_tasks == LocalTasks::Unasked {
                let _ = self.run("tasks.sendLocal", json!({"account": null}));
            }
        }
<<<<<<< HEAD
        // A question that went away unanswered is not asked again by itself.
        let asking: Vec<Id> = self.tasks.perm.iter().filter(|(_, p)| **p == Permission::Asking).map(|(a, _)| *a).collect();
        let still = self.signing_in.is_some()
            || matches!(&self.dialog, Some(Dialog::Confirm { command, .. }) if command == "ui.allowTasks" || command == "ui.connectGoogleTasks");
        if !still {
            for account in asking {
                self.tasks.perm.insert(account, Permission::Declined);
=======
        // (A question that went away unanswered is a no: `sync_ask_tick`.)
        self.sync_ask_tick();
        // Who is asked next is the engine's to say.
        match self.session.tasks_question(&self.ask_window()) {
            Some(TasksQuestion::Permission(account)) => {
                self.ask_sync(account);
>>>>>>> upstream/main
            }
            Some(TasksQuestion::LocalTasks) => {
                self.offer_local_tasks();
            }
            None => {}
        }
    }

    /// For a stand-in host (the screenshot example): every account that keeps to-dos has its To Do
    /// working, as if a job had read it.
    #[doc(hidden)]
    pub fn stand_in_tasks_work(&mut self) {
        let accounts = tasks_remote::task_accounts(&self.session.mailbox);
        self.session.driver.tasks.working_all(&accounts);
    }

    /// The accounts whose To Do is working (a job read it with the permission).
    fn tasks_working(&self) -> Vec<Id> {
        self.session.tasks_working()
    }

    /// Whether a to-do put in `account` waits on this computer because a job found that To Do
    /// isn't allowed there (nothing is known of an account no job has looked at yet).
    pub(crate) fn tasks_held(&self, account: Id) -> bool {
        self.session.tasks_held(account)
    }

    /// The first account added whose To Do works, wherever the owner moved it (#702): File › Info
    /// says what is local under it, and the question offers it first.
    pub(crate) fn tasks_working_first(&self) -> Option<Id> {
        let working = self.tasks_working();
        self.session.mailbox.accounts.iter().map(|a| a.id).find(|id| working.contains(id))
    }

    /// How many to-dos exist only on this computer and could be offered to an account's To Do;
    /// 0 when no account's To Do works yet.
    pub(crate) fn local_tasks_to_offer(&self) -> usize {
        if self.tasks_working().is_empty() { 0 } else { tasks_remote::local_task_count(&self.session.mailbox) }
    }

    /// Ask the owner, once, whether the to-dos that exist only here are to go to an account's To
    /// Do. Nothing is sent before a yes. (File › Info asks again.) Returns whether it asked.
    pub(crate) fn offer_local_tasks(&mut self) -> bool {
        let count = self.local_tasks_to_offer();
        let accounts = self.tasks_working();
        if count == 0 || self.dialog.is_some() || self.signing_in.is_some() {
            return false;
        }
        let address = |id: Id| self.session.mailbox.account(id).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let Some(first) = self.tasks_working_first() else { return false };
        let how_many = tln!(count, "Send your {n} local task to {address}'s To Do?", "Send your {n} local tasks to {address}'s To Do?");
        let title = crate::i18n::fmt(&how_many, &[("n", &count.to_string()), ("address", &address(first))]);
        let more_than_one = accounts.len() > 1;
        let text = if more_than_one {
            tl!(
                "These tasks exist only on this computer. Choose the Microsoft account whose To Do gets them. Until you choose, they stay here and nothing of them is sent."
            )
        } else {
            tl!(
                "These tasks exist only on this computer. If you send them, they are kept in step with Microsoft To Do. Until you do, they stay here and nothing of them is sent. File › Info offers this again."
            )
        };
        let label = |id: Id| crate::i18n::fmt(tl!("Send to {address}"), &[("address", &address(id))]);
        let mut ask = Question {
            title: if more_than_one { tl!("Send Your Local Tasks to To Do?").to_string() } else { title },
            text: text.to_string(),
            action: if more_than_one { label(first) } else { tl!("Send Tasks").to_string() },
            with: json!({"account": first}),
            more: Vec::new(),
            note: None,
            // Closing it is the same as Not now (`tasks_ask_tick`), so there is no other Cancel.
            cancel: Some(tl!("Not now").to_string()),
            tick: None,
            calendar: None,
            cut: None,
            listing: None,
        };
        for id in accounts.iter().filter(|id| **id != first) {
            ask.more.push((label(*id), json!({"account": id})));
        }
        self.tasks.offer_open = true;
        self.set_dialog(Some(Dialog::Confirm { command: "tasks.sendLocal".to_string(), params: json!({}), ask: Box::new(ask) }));
        true
    }

    /// What File › Info says about the to-dos that exist only here, and whether it can offer to
    /// send them: `None` when there is nothing to say.
    pub(crate) fn local_tasks_line(&self) -> Option<(String, bool)> {
        let count = self.local_tasks_to_offer();
        if count == 0 {
            return None;
        }
        let line = tln!(count, "{n} task exists only on this computer", "{n} tasks exist only on this computer");
        Some((line, true))
    }

<<<<<<< HEAD
    /// The question before Microsoft's page: what is asked of the owner and why. (Google's, for an
    /// account linked to Google Tasks: [`MailApp::ask_google_tasks`].)
    pub(crate) fn ask_tasks(&mut self, account: Id) -> bool {
        if tasks_remote::keeps_google_tasks(&self.session.mailbox, account) {
            return self.ask_google_tasks(account);
        }
        let Some(address) = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()) else { return false };
        let text = crate::i18n::fmt(
            tl!(
                "To keep the tasks of {address} in step with Microsoft To Do, MailCraft needs your permission to read and change your To Do lists and tasks. Microsoft's page opens in your browser to ask you, once.\n\nUntil you allow it, tasks stay on this computer and nothing of them is sent to Microsoft."
            ),
            &[("address", &address)],
        );
        let ask = Question {
            title: tl!("Keep Tasks in Step with Microsoft To Do").to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
            cancel: None,
            tick: None,
=======
    /// The one question is put ([`crate::server_sync`]): the to-dos that wait for it are being
    /// asked about.
    pub(crate) fn tasks_sync_asking(&mut self, account: Id) {
        if matches!(self.session.driver.tasks.perm.get(&account), Some(Permission::Needed | Permission::Declined | Permission::Refused(_))) {
            self.session.driver.tasks.perm.insert(account, Permission::Asking);
        }
    }

    /// The one question went away unanswered: the to-dos stay here, and nobody is asked again by
    /// itself (File › Info has Ask again).
    pub(crate) fn tasks_sync_not_now(&mut self, account: Id) {
        if matches!(self.session.driver.tasks.perm.get(&account), Some(Permission::Needed | Permission::Asking)) {
            self.session.driver.tasks.perm.insert(account, Permission::Declined);
        }
    }

    /// The page signed in somebody else: the to-dos wait as they did.
    pub(crate) fn tasks_sync_waits(&mut self, account: Id) {
        if self.session.driver.tasks.perm.get(&account) == Some(&Permission::Asking) {
            self.session.driver.tasks.perm.insert(account, Permission::Needed);
        }
    }

    /// Asked, and no: why, in a few words.
    pub(crate) fn tasks_sync_refused(&mut self, account: Id, why: &str) {
        if matches!(self.session.driver.tasks.perm.get(&account), Some(Permission::Needed | Permission::Asking | Permission::Declined)) {
            self.session.driver.tasks.perm.insert(account, Permission::Refused(why.to_string()));
        }
    }

    /// The owner allowed it on Microsoft's page: the next job finds out, and a refusal right after
    /// is taken for Microsoft not having taken it in yet (#491).
    pub(crate) fn tasks_allowed(&mut self, account: Id) {
        self.session.driver.tasks.allowed(account);
    }

    /// The to-dos' part of File › Info's line about keeping in step with Outlook.
    pub(crate) fn tasks_sync_part(&self, account: Id) -> Option<(String, bool, crate::server_sync::Perm)> {
        use crate::server_sync::Perm;
        let (line, again) = self.tasks_line(account)?;
        let perm = match self.session.driver.tasks.perm.get(&account) {
            Some(Permission::Asking) => Perm::Asking,
            Some(Permission::Given) | None => Perm::Fine,
            Some(_) => Perm::Waits,
>>>>>>> upstream/main
        };
        Some((line, again, perm))
    }

    /// What File › Info says under an account's card about its tasks, and whether there is a way
    /// to ask again. `None` for an account whose tasks no server keeps.
    pub(crate) fn tasks_line(&self, account: Id) -> Option<(String, bool)> {
        self.services.graph_tasks.as_ref()?;
        if !tasks_remote::task_accounts(&self.session.mailbox).contains(&account) {
            return None;
        }
<<<<<<< HEAD
        if tasks_remote::keeps_google_tasks(&self.session.mailbox, account) {
            return Some(self.google_tasks_line(account));
        }
        Some(match self.tasks.perm.get(&account) {
=======
        Some(match self.session.driver.tasks.perm.get(&account) {
>>>>>>> upstream/main
            Some(Permission::Given) => (tl!("Tasks: kept in step with Microsoft To Do").to_string(), false),
            Some(Permission::Needed) => (tl!("Tasks: waiting for your permission").to_string(), true),
            Some(Permission::Asking) => (tl!("Tasks: asking for your permission…").to_string(), false),
            Some(Permission::Declined | Permission::Denied) => (tl!("Tasks: kept on this computer only (not allowed yet)").to_string(), true),
            Some(Permission::Refused(why)) => {
                let why = crate::forms::kit::shown(why, crate::server_sync::REASON_SHOWN);
                (crate::i18n::fmt(tl!("Tasks: not allowed ({why})"), &[("why", why.as_str())]), true)
            }
            Some(Permission::SignInOver(_)) => (tl!("Tasks: Microsoft no longer takes the sign-in").to_string(), true),
            None => (tl!("Tasks: not asked yet").to_string(), false),
        })
    }
}

/// Proddyt Labs fork: Google Tasks, for an account whose to-dos Microsoft doesn't keep.
impl MailApp {
    /// Whether File › Info offers to connect the account's to-dos to Google Tasks: an account of
    /// the owner's own (not a shared mailbox) whose to-dos no server keeps yet, at Google (Gmail's
    /// servers, a Google sign-in, or a Gmail address).
    pub(crate) fn offers_google_tasks(&self, account: Id) -> bool {
        let Some(a) = self.session.mailbox.account(account) else { return false };
        let at_google = a.is_gmail()
            || a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Google)
            || a.email.trim().rsplit_once('@').is_some_and(|(_, domain)| mailcraft_model::gmail::is_gmail_host(domain));
        self.services.graph_tasks.is_some() && !a.is_shared() && !a.keeps_tasks() && a.kind.has_server() && at_google
    }

    /// What File › Info says of Google Tasks for an account it offers it to
    /// ([`MailApp::offers_google_tasks`]), and whether it can be connected from here: a build
    /// without a Google registration says why not.
    pub(crate) fn google_tasks_offer(&self, account: Id) -> Option<(String, bool)> {
        if !self.offers_google_tasks(account) {
            return None;
        }
        if !self.google_registration_ready() {
            return Some((tl!("Google Tasks: not available in this build (it has no Google registration)").to_string(), false));
        }
        Some((tl!("Google Tasks: not connected").to_string(), true))
    }

    /// What File › Info says of an account linked to Google Tasks, and whether it can be connected
    /// again from here.
    fn google_tasks_line(&self, account: Id) -> (String, bool) {
        if !self.google_registration_ready() {
            return (tl!("Tasks: Google Tasks can't be reached from this build (it has no Google registration)").to_string(), false);
        }
        match self.tasks.perm.get(&account) {
            Some(Permission::Given) => (tl!("Tasks: kept in step with Google Tasks").to_string(), false),
            Some(Permission::Asking) => (tl!("Tasks: waiting for Google's page…").to_string(), false),
            Some(Permission::Refused(why)) => {
                (crate::i18n::fmt(tl!("Tasks: Google Tasks not connected ({why})"), &[("why", crate::forms::kit::shown(why, 90).as_str())]), true)
            }
            Some(Permission::SignInOver(_) | Permission::Needed) => (tl!("Tasks: Google Tasks needs you to connect again").to_string(), true),
            Some(Permission::Declined) => (tl!("Tasks: kept on this computer only (Google Tasks not connected)").to_string(), true),
            None if self.tasks_refresh(account).is_none() => (tl!("Tasks: Google Tasks not connected yet").to_string(), true),
            None => (tl!("Tasks: Google Tasks, not read yet").to_string(), false),
        }
    }

    /// The question before Google's page: what is asked of the owner and why. Nothing is sent
    /// before a yes.
    pub(crate) fn ask_google_tasks(&mut self, account: Id) -> bool {
        let Some(address) = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()) else { return false };
        if self.dialog.is_some() || self.signing_in.is_some() {
            return false;
        }
        let text = crate::i18n::fmt(
            tl!(
                "To keep the tasks of {address} in step with Google Tasks, MailCraft needs your permission to read and change your Google Tasks lists and tasks. Google's page opens in your browser to ask you; choose {address} there.\n\nTasks made here or in Google Tasks then appear in both, and so do changes and tasks marked complete. Until you allow it, nothing of your tasks is sent to Google."
            ),
            &[("address", &address)],
        );
        let ask = Question {
            title: tl!("Connect Google Tasks").to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
            cancel: None,
            tick: None,
        };
        self.dialog = Some(Dialog::Confirm { command: "ui.connectGoogleTasks".to_string(), params: json!({"account": account}), ask });
        true
    }

    /// `ui.connectGoogleTasks`: Google's page, for the owner's Google Tasks alone, because the
    /// owner said go ahead.
    pub(crate) fn begin_google_tasks_sign_in(&mut self, account: Id) -> Result<Value, String> {
        let owner = self.session.mailbox.account(account).filter(|a| !a.is_shared() && !a.keeps_microsoft_tasks() && a.kind.has_server()).cloned();
        let owner = owner.ok_or_else(|| tl!("This account's tasks can't be kept in step with Google Tasks").to_string())?;
        if !self.google_registration_ready() {
            return Err(tl!(
                "This build of MailCraft has no Google registration, so Google Tasks can't be connected. It is built with one when MAILCRAFT_GOOGLE_CLIENT_ID and MAILCRAFT_GOOGLE_CLIENT_SECRET are set."
            )
            .to_string());
        }
        let google = mailcraft_model::OAuth { provider: mailcraft_model::OAuthProvider::Google, ..Default::default() };
        self.google_tasks_next = true;
        let begun = self.begin_sign_in(owner.id, google, &owner.email, String::new(), false, false);
        self.google_tasks_next = false;
        begun?;
        self.tasks.perm.insert(account, Permission::Asking);
        self.session.sync_status =
            tl!("Allow MailCraft to keep your tasks in step with Google Tasks on the page that opened in your browser…").to_string();
        Ok(json!({"pending": "signIn", "account": owner.id}))
    }

    /// The owner answered on Google's page about Google Tasks: with a lasting token for the
    /// account's own Google account, it is linked and its tasks are kept in step at once; without,
    /// why not is said.
    pub(crate) fn google_tasks_signed_in(&mut self, account: Id, answer: Result<Tokens, String>) {
        use crate::approval::{GOOGLE_SIGN_IN_BLOCKED, NO_BROWSER};
        self.session.sync_status.clear();
        match answer {
            // (Somebody else's tokens are neither used nor remembered, #103; Google always says who.)
            Ok(tokens) if self.signed_in_as_other(account, &tokens) => {
                self.tasks.perm.insert(account, Permission::Needed);
            }
            Ok(tokens) if tokens.refresh.is_empty() => {
                let said = tl!("Google didn't give MailCraft a lasting permission for Google Tasks: connect again");
                self.tasks.perm.insert(account, Permission::Refused(said.to_string()));
                self.status(said);
            }
            Ok(tokens) => {
                self.remember(GOOGLE_TASKS_SECRET, account, &tokens.refresh);
                self.google_tasks_refresh.insert(account, tokens.refresh.clone());
                if let Err(e) = self.run("tasks.linkGoogle", json!({"account": account, "linked": true})) {
                    self.status(crate::i18n::fmt(tl!("Google Tasks wasn't connected: {error}"), &[("error", &e)]));
                    return;
                }
                log::info!("tasks, account {account}: Google Tasks connected; the next job reads it");
                self.tasks.perm.remove(&account);
                self.tasks.settling.insert(account, SETTLING_TRIES);
                self.tasks.book.worked(account);
                self.tasks.book.forget_checked(account);
                self.status(tl!("Google Tasks connected: your tasks are kept in step with it"));
            }
            Err(e) if e == GOOGLE_SIGN_IN_BLOCKED => {
                let said = tl!("Google's policy doesn't let MailCraft use Google Tasks for this account");
                self.tasks.perm.insert(account, Permission::Refused(said.to_string()));
                self.status(said);
            }
            Err(e) if e == NO_BROWSER => {
                self.tasks.perm.insert(account, Permission::Declined);
                self.status(crate::i18n::fmt(tl!("Tasks stay on this computer for now: {error}"), &[("error", &e)]));
            }
            Err(e) => {
                let said = crate::i18n::fmt(tl!("Google Tasks wasn't connected: {error}"), &[("error", &e)]);
                self.tasks.perm.insert(account, Permission::Refused(said.clone()));
                self.status(said);
            }
        }
    }

    /// File › Info's Disconnect: the account's to-dos stay on this computer, Google is told
    /// nothing, and its sign-in for Google Tasks is forgotten.
    pub(crate) fn disconnect_google_tasks(&mut self, account: Id) {
        let _ = self.run("tasks.linkGoogle", json!({"account": account, "linked": false}));
        self.google_tasks_refresh.remove(&account);
        self.tasks.perm.remove(&account);
        if let Some(a) = self.session.mailbox.account(account).cloned() {
            self.forget_account_secret(GOOGLE_TASKS_SECRET, &a);
        }
    }
}
