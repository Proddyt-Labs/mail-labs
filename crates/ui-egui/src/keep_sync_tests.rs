// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Google Keep sign-in in the window, against a stand-in credential store.
//! A connected sign-in is kept in the store (and this session's memory) and nowhere else; a
//! disconnect deletes it; the cookie the owner typed leaves the box and is kept nowhere. Every
//! token here is made up.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_host::{SecretError, SecretStore};
use mailcraft_model::keep::Secret;
use serde_json::json;

use super::*;
use crate::keep_sync::{KEEP_SECRET, KeepConnectJob, KeepConnectResult};

const MASTER: &str = "aas_et/STANDIN-KEEP-MASTER-0001";
const COOKIE: &str = "oauth2_4/STANDIN-KEEP-COOKIE-0002";

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

impl Store {
    fn values(&self) -> Vec<String> {
        self.0.lock().unwrap().values().cloned().collect()
    }
}

fn app(store: &Store) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    app.ui.remember_sign_in = true;
    app.services.secrets = Some(Arc::new(store.clone()));
    app
}

fn google(app: &mut MailApp) -> Id {
    app.execute(
        "account.add",
        json!({
            "email": "owner@example.org", "auth": "oauth2", "provider": "google",
            "incoming": {"host": "imap.gmail.com", "port": 993, "security": "tls"},
            "outgoing": {"host": "smtp.gmail.com", "port": 465, "security": "tls"}
        }),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap()
}

#[test]
fn a_connected_sign_in_is_kept_in_the_store_only_and_disconnecting_deletes_it() {
    let store = Store::default();
    let mut app = app(&store);
    let account = google(&mut app);
    app.run("keep.link", json!({"account": account, "linked": true})).unwrap();
    app.remember(KEEP_SECRET, account, MASTER);
    app.gkeep_master.insert(account, Secret::new(MASTER.into()));
    assert!(store.values().iter().any(|v| v == MASTER), "the sign-in is in the credential store");
    // Not in the mailbox that is saved, nor in what the window keeps of it.
    let saved = serde_json::to_string(&app.session.mailbox).unwrap();
    assert!(!saved.contains(MASTER), "the mailbox holds no sign-in");

    app.disconnect_google_keep(account);
    assert!(store.values().iter().all(|v| v != MASTER), "disconnecting deletes the credential");
    assert!(app.gkeep_master.is_empty(), "and the copy in memory");
    assert!(!app.session.mailbox.account(account).unwrap().google_keep, "the account is no longer linked");
    let saved = serde_json::to_string(&app.session.mailbox).unwrap();
    assert!(!saved.contains(MASTER));
}

#[test]
fn the_cookie_leaves_the_box_when_traded_and_is_kept_nowhere() {
    let store = Store::default();
    let mut app = app(&store);
    let account = google(&mut app);
    let traded = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&traded);
    app.services.keep_connect = Some(Box::new(move |job: KeepConnectJob| {
        sink.lock().unwrap().push(job.cookie.expose().to_string());
        // (The answer never comes in this test: the job is only started.)
        let (tx, rx) = mpsc::channel::<KeepConnectResult>();
        std::mem::forget(tx);
        rx
    }));
    let mut cookie = COOKIE.to_string();
    app.begin_keep_connect(account, &mut cookie).unwrap();
    assert!(cookie.is_empty(), "the box is cleared once the cookie is taken");
    assert_eq!(traded.lock().unwrap().as_slice(), [COOKIE.to_string()], "the job was given the cookie, once");
    assert!(app.gkeep.connecting(), "the trade is under way");
    let saved = serde_json::to_string(&app.session.mailbox).unwrap();
    assert!(!saved.contains(COOKIE), "the mailbox holds no cookie");
    assert!(store.values().iter().all(|v| !v.contains(COOKIE)), "the store holds no cookie");
    assert!(app.status_msg.as_ref().is_none_or(|(msg, _)| !msg.contains(COOKIE)), "nothing shows the cookie");
}
