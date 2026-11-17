// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Google Keep jobs from end to end, against a stand-in for Keep: the
//! sign-in traded, notes read and merged into a mailbox by the engine, changed here, and sent
//! back; and what the jobs show of the secrets they hold (nothing). Every token here is made up.

use std::cell::RefCell;

use mailcraft_engine::cmd::keep::{keep_upload, merge_keep};
use mailcraft_model::keep::Secret;
use mailcraft_model::{Account, AccountKind, Id, Mailbox};
use mailcraft_net::NetError;
use mailcraft_net::google_keep::sync_with;
use serde_json::{Value, json};

use crate::keep::{KeepConnectJob, KeepJob, KeepResult, connect_with, run_with, scrub};

const MASTER: &str = "aas_et/STANDIN-MASTER-0001";
const ACCESS: &str = "ya29.standin-access-0002";
const COOKIE: &str = "oauth2_4/standin-cookie-0003";
const OWNER: &str = "owner@example.org";

fn mailbox() -> (Mailbox, Id) {
    let mut mailbox = Mailbox::default();
    let account = mailbox.add_account(Account { email: OWNER.into(), kind: AccountKind::Imap, google_keep: true, ..Default::default() });
    (mailbox, account)
}

fn job(mailbox: &Mailbox, account: Id, upload: mailcraft_model::keep::KeepUpload) -> KeepJob {
    KeepJob { account: mailbox.account(account).cloned().unwrap(), master: Secret::new(MASTER.into()), upload }
}

fn renewed(email: &str, master: &Secret) -> Result<Secret, NetError> {
    assert_eq!((email, master.expose()), (OWNER, MASTER));
    Ok(Secret::new(ACCESS.into()))
}

/// Keep, as a stand-in: one note with a body, then whatever is sent is echoed back.
fn keep_answer(body: &str, seen: &RefCell<Vec<Value>>) -> Result<(u16, Vec<u8>), NetError> {
    let sent: Value = serde_json::from_str(body).unwrap();
    seen.borrow_mut().push(sent.clone());
    let mut nodes = vec![
        json!({"id": "n1", "serverId": "n1", "kind": "notes#node", "type": "NOTE", "parentId": "root", "sortValue": "1", "title": "Shopping",
            "color": "DEFAULT", "timestamps": {"updated": "2026-02-01T10:00:00.000Z", "created": "2026-01-01T10:00:00.000Z"}}),
        json!({"id": "b1", "serverId": "b1", "kind": "notes#node", "type": "LIST_ITEM", "parentId": "n1", "sortValue": "1", "text": "bread",
            "timestamps": {"updated": "2026-02-01T10:00:00.000Z"}}),
    ];
    // (What was sent comes back as Keep's, with its own times.)
    for node in sent["nodes"].as_array().cloned().unwrap_or_default() {
        nodes.push(node);
    }
    Ok((200, json!({"toVersion": "v1", "truncated": false, "nodes": nodes}).to_string().into_bytes()))
}

#[test]
fn a_job_reads_the_notes_into_the_mailbox_and_then_sends_what_was_changed() {
    let (mut mailbox, account) = mailbox();
    let seen = RefCell::new(Vec::new());
    let sync = |access: &Secret, upload: &_| {
        assert_eq!(access.expose(), ACCESS);
        sync_with(&|body| keep_answer(body, &seen), upload, 1_800_000_000, "s--1--2")
    };
    let read = run_with(job(&mailbox, account, keep_upload(&mailbox, account).unwrap()), &renewed, &sync);
    assert!(read.errors.is_empty() && !read.sign_in_refused && !read.denied);
    let download = read.download.unwrap();
    assert_eq!((download.account, download.email.as_str(), download.version.as_str(), download.full), (account, OWNER, "v1", true));
    merge_keep(&mut mailbox, &download).unwrap();
    let note = mailbox.notes.values().find(|n| n.title == "Shopping").unwrap().clone();
    assert_eq!(note.text, "bread");
    // Changed here, sent by the next job: Keep sees the new text in the body node, with its version of the note.
    let id = note.id;
    let mut session = mailcraft_engine::Session::at(mailbox.clone(), 1_900_000_000, 0);
    session.run("notes.update", &json!({"id": id, "text": "bread and milk"})).unwrap();
    let mailbox = session.mailbox.clone();
    let upload = keep_upload(&mailbox, account).unwrap();
    assert_eq!(upload.version, "v1");
    let sent = run_with(job(&mailbox, account, upload), &renewed, &sync);
    assert!(sent.errors.is_empty());
    let body = seen.borrow().last().unwrap().clone();
    assert_eq!(body["targetVersion"], "v1");
    let texts: Vec<&str> = body["nodes"].as_array().unwrap().iter().filter_map(|n| n["text"].as_str()).collect();
    assert!(texts.contains(&"bread and milk"), "{texts:?}");
    // The access token is in a header of the real client, never in the body that Keep is sent.
    assert!(!body.to_string().contains(ACCESS) && !body.to_string().contains(MASTER));
}

#[test]
fn a_refused_sign_in_asks_to_connect_again_and_trouble_says_a_line_without_the_secrets() {
    let (mailbox, account) = mailbox();
    let never_sync = |_: &Secret, _: &_| -> Result<mailcraft_model::keep::KeepDownload, NetError> { panic!("no access token, no sync") };
    // Google no longer takes the master token.
    let refused = |_: &str, _: &Secret| Err(mailcraft_net::google_keep::sync(&Secret::new(String::new()), &Default::default()).unwrap_err());
    let out = run_with(job(&mailbox, account, Default::default()), &refused, &never_sync);
    assert!(out.sign_in_refused && out.download.is_none() && out.errors.is_empty());
    // A network error that happens to carry the secrets: they are blanked out of the line.
    let leaky = |_: &str, _: &Secret| Err(NetError::Config(format!("went wrong with {MASTER} and {ACCESS}").into()));
    let out = run_with(job(&mailbox, account, Default::default()), &leaky, &never_sync);
    assert_eq!(out.errors.len(), 1);
    assert!(!out.errors[0].contains("STANDIN") && !out.errors[0].contains(MASTER), "{}", out.errors[0]);
    let leaky_sync = |access: &Secret, _: &_| Err(NetError::Config(format!("went wrong with {MASTER} and {}", access.expose()).into()));
    let out = run_with(job(&mailbox, account, Default::default()), &renewed, &leaky_sync);
    assert!(!out.errors[0].contains("STANDIN") && !out.errors[0].contains("standin-access"), "{}", out.errors[0]);
    // Keep says no for the account: said, not retried.
    let forbidden = |_: &Secret, _: &_| Err(NetError::Forbidden("no".into()));
    assert!(run_with(job(&mailbox, account, Default::default()), &renewed, &forbidden).denied);
}

#[test]
fn nothing_a_job_or_its_result_holds_shows_a_secret() {
    let (mailbox, account) = mailbox();
    let k = job(&mailbox, account, Default::default());
    let connect = KeepConnectJob { account: mailbox.account(account).cloned().unwrap(), cookie: Secret::new(COOKIE.into()) };
    let result = KeepResult { account, errors: vec!["x".into()], ..Default::default() };
    let done = crate::keep::KeepConnectResult { account, master: Some(Secret::new(MASTER.into())), error: None };
    for shown in [
        format!("{k:?}"),
        format!("{k:#?}"),
        format!("{connect:?}"),
        format!("{connect:#?}"),
        format!("{result:?}"),
        format!("{done:?}"),
        format!("{done:#?}"),
    ] {
        for secret in [MASTER, ACCESS, COOKIE, "STANDIN", "standin"] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }
    assert_eq!(scrub(&format!("a {MASTER} b"), &[MASTER]), "a [hidden] b");
}

#[test]
fn connecting_hands_back_the_master_token_or_a_sentence_that_never_holds_the_cookie() {
    let (mailbox, account) = mailbox();
    let who = || KeepConnectJob { account: mailbox.account(account).cloned().unwrap(), cookie: Secret::new(COOKIE.into()) };
    let done = connect_with(who(), &|email, cookie| {
        assert_eq!((email, cookie.as_str()), (OWNER, COOKIE));
        cookie.clear();
        Ok(Secret::new(MASTER.into()))
    });
    assert_eq!(done.master.as_ref().map(Secret::expose), Some(MASTER));
    assert!(done.error.is_none());
    // The exchange's own error writes the cookie back: the line is scrubbed.
    let done = connect_with(who(), &|_, cookie| Err(NetError::Config(format!("bad {cookie}").into())));
    assert!(done.master.is_none());
    assert!(!done.error.unwrap().contains(COOKIE));
}
