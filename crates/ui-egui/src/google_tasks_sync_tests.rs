// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: Google Tasks in the window: offered for a Gmail account kept by IMAP,
//! connected on Google's page by itself, and then kept in step by the same job against Google.
//! Hosts are fakes; every address and value is invented.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, OAuthProvider};
use serde_json::json;

use crate::task_sync::{TasksJob, TasksResult};
use crate::{MailApp, Services, SignInRequest};

const OWNER: &str = "owner@gmail.example";
const NOW: i64 = 1_791_993_600;

type Jobs = Arc<Mutex<Vec<(OAuthProvider, String, String)>>>;
type SignIns = Arc<Mutex<Vec<(Id, bool, bool, String)>>>;

fn window(signed_in_as: &'static str) -> (MailApp, Id, Jobs, SignIns) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let account = app.execute("account.add", json!({"email": OWNER, "incoming": {"host": "imap.gmail.com"}})).unwrap()["id"].as_u64().unwrap();
    let (jobs, sign_ins): (Jobs, SignIns) = Default::default();
    let log = jobs.clone();
    app.services.graph_tasks = Some(Box::new(move |job: TasksJob| {
        log.lock().unwrap().push((job.oauth.provider, job.refresh.clone(), job.google_client_secret.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(TasksResult { account: job.account.id, ..Default::default() }).unwrap();
        rx
    }));
    let log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.google_tasks, req.graph, req.google_client_secret.clone()));
        let (tx, rx) = mpsc::channel();
        let tokens = Tokens {
            access: "g-access".into(),
            refresh: "g-refresh".into(),
            email: signed_in_as.into(),
            expires_at: mailcraft_time::now() + 3_600,
            ..Default::default()
        };
        tx.send(Ok(tokens)).unwrap();
        rx
    }));
    (app, account, jobs, sign_ins)
}

fn registered(app: &mut MailApp) {
    app.services.google_client_id = "123-abc.apps.example".into();
    app.services.google_client_secret = "installed-app-value".into();
}

#[test]
fn a_build_without_a_google_registration_says_so_and_asks_nothing() {
    let (mut app, account, _, sign_ins) = window(OWNER);
    let (line, can) = app.google_tasks_offer(account).unwrap();
    assert!(!can && line.contains("no Google registration"), "{line}");
    let said = app.begin_google_tasks_sign_in(account).unwrap_err();
    assert!(said.contains("MAILCRAFT_GOOGLE_CLIENT_ID") && said.contains("MAILCRAFT_GOOGLE_CLIENT_SECRET"), "{said}");
    assert!(sign_ins.lock().unwrap().is_empty());
}

#[test]
fn connecting_links_the_account_and_the_job_runs_against_google() {
    let (mut app, account, jobs, sign_ins) = window(OWNER);
    registered(&mut app);
    assert_eq!(app.google_tasks_offer(account).map(|o| o.1), Some(true));
    app.begin_google_tasks_sign_in(account).unwrap();
    app.poll_sign_in();
    let asked = sign_ins.lock().unwrap().clone();
    assert_eq!(asked, [(account, true, false, "installed-app-value".to_string())]);
    assert!(app.session.mailbox.account(account).unwrap().keeps_google_tasks());
    assert_eq!(app.google_tasks_refresh.get(&account).map(String::as_str), Some("g-refresh"));
    // Linked: no longer offered, and File › Info says where it stands.
    assert!(app.google_tasks_offer(account).is_none());
    assert!(app.tasks_line(account).is_some());
    assert!(app.tasks_tick_at(0.0));
    let ran = jobs.lock().unwrap().clone();
    assert_eq!(ran, [(OAuthProvider::Google, "g-refresh".to_string(), "installed-app-value".to_string())]);
    // The mail sign-in was never touched.
    assert!(app.tokens.get(&account).is_none());

    app.poll_tasks();
    app.disconnect_google_tasks(account);
    assert!(!app.session.mailbox.account(account).unwrap().keeps_tasks() && app.google_tasks_refresh.is_empty());
}

#[test]
fn another_google_account_chosen_on_the_page_links_nothing() {
    let (mut app, account, jobs, _) = window("someone.else@gmail.example");
    registered(&mut app);
    app.begin_google_tasks_sign_in(account).unwrap();
    app.poll_sign_in();
    assert!(!app.session.mailbox.account(account).unwrap().keeps_tasks());
    assert!(app.google_tasks_refresh.is_empty());
    assert!(!app.tasks_tick_at(0.0) && jobs.lock().unwrap().is_empty());
}
