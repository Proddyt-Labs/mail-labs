// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Google Tasks job from end to end, against answers recorded in the shape
//! Google gives them (`mailcraft-net`'s `testdata/google-tasks.json`): read, merged into a mailbox
//! by the engine, changed here, and sent back. Nothing here leaves this machine.

use std::cell::RefCell;

use mailcraft_engine::cmd::tasks_remote::{apply_task_outcomes, merge_tasks, task_upload};
use mailcraft_model::remote_tasks::{TaskOpKind, TaskResult, TasksUpload};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, AccountKind, Id, Mailbox, OAuth, OAuthProvider};
use mailcraft_net::NetError;
use mailcraft_net::google_tasks::{push_tasks_with, tasks_with};
use serde_json::{Value, json};

use crate::tasks::{TasksJob, run_google_with};

const RECORDED: &str = include_str!("../../net/testdata/google-tasks.json");
const OWNER: &str = "owner@gmail.example";

/// The recorded answers to the reads, by `"METHOD path"`.
fn replay(method: &str, path: &str, _body: Option<&str>) -> Result<(u16, Vec<u8>), NetError> {
    let recorded: Value = serde_json::from_str(RECORDED).unwrap();
    match recorded.get(format!("{method} {path}")) {
        Some(answer) => Ok((200, answer.to_string().into_bytes())),
        None => Ok((404, b"{}".to_vec())),
    }
}

/// A mailbox with a Gmail account kept by IMAP with an app password, linked to Google Tasks.
fn mailbox() -> (Mailbox, Id) {
    let mut mailbox = Mailbox::default();
    let account = mailbox.add_account(Account { email: OWNER.into(), kind: AccountKind::Imap, google_tasks: true, ..Default::default() });
    (mailbox, account)
}

fn job(mailbox: &Mailbox, account: Id, upload: TasksUpload) -> TasksJob {
    TasksJob {
        account: mailbox.account(account).cloned().unwrap(),
        oauth: OAuth { provider: OAuthProvider::Google, client_id: "id.apps.example".into(), ..Default::default() },
        refresh: "refresh".into(),
        upload,
        zone: mailcraft_time::Zone::UTC,
        cancel: Default::default(),
        google_client_secret: "installed-app".into(),
    }
}

fn renewed(_: &OAuth, refresh: &str) -> Result<Tokens, NetError> {
    assert_eq!(refresh, "refresh");
    Ok(Tokens { access: "access".into(), refresh: "newer".into(), ..Default::default() })
}

#[test]
fn the_recorded_lists_come_into_the_mailbox_once() {
    let (mut mailbox, account) = mailbox();
    let read = |token: &str| {
        assert_eq!(token, "access");
        tasks_with(&replay)
    };
    let out = run_google_with(job(&mailbox, account, TasksUpload::default()), &renewed, &|_, _| unreachable!("nothing to send"), &read);
    assert!(out.errors.is_empty() && !out.sign_in_refused && !out.consent_needed);
    assert_eq!(out.tokens.as_ref().map(|t| t.refresh.as_str()), Some("newer"));
    let download = out.download.unwrap();
    assert_eq!((download.account, download.email.as_str()), (account, OWNER));

    let merged = merge_tasks(&mut mailbox, &download).unwrap();
    assert_eq!((merged.added, merged.updated, merged.removed), (5, 0, 0));
    let titled = |title: &str| mailbox.tasks.values().find(|t| t.title == title).unwrap().clone();
    let child = titled("Comprar microfone");
    assert_eq!(child.categories, ["Studio"]);
    assert_eq!(child.notes, "↳ [Studio] Gravar episódio");
    assert_eq!(child.origin.account, account);
    let closed = titled("Reunião com a gráfica");
    assert!(closed.is_complete() && closed.categories == ["Fechada"]);
    assert_eq!(titled("Editar vídeo").categories, ["Em andamento", "Studio"]);
    assert_eq!(titled("Revisar contrato").list, "MDJTdHVkaW8");

    // Nothing was changed here: nothing waits to go, and reading it again changes nothing.
    assert!(task_upload(&mailbox, account).unwrap().ops.is_empty());
    let download = tasks_with(&replay).unwrap().for_account(mailbox.account(account).unwrap());
    let again = merge_tasks(&mut mailbox, &download).unwrap();
    assert_eq!((again.added, again.updated, again.removed, again.unchanged), (0, 0, 0, 5));
}

#[test]
fn what_is_done_here_goes_to_google_and_comes_back_the_same() {
    let (mut mailbox, account) = mailbox();
    let download = tasks_with(&replay).unwrap().for_account(mailbox.account(account).unwrap());
    merge_tasks(&mut mailbox, &download).unwrap();
    let id_of = |mailbox: &Mailbox, title: &str| mailbox.tasks.values().find(|t| t.title == title).unwrap().id;

    // Done here, a tag added to another, a subtask's notes written, and a new one made.
    let record = id_of(&mailbox, "Gravar episódio");
    mailbox.task_mut(record).unwrap().completed = Some(1_790_000_000);
    let edit = id_of(&mailbox, "Editar vídeo");
    mailbox.task_mut(edit).unwrap().categories.push("Urgente".into());
    let child = id_of(&mailbox, "Comprar microfone");
    mailbox.task_mut(child).unwrap().notes = "↳ [Studio] Gravar episódio\nShure ou Rode".into();
    mailbox.add_task(mailcraft_model::Task {
        title: "Novo roteiro".into(),
        categories: vec!["Studio".into(), "A fazer".into()],
        origin: mailcraft_model::Origin { account, ..Default::default() },
        ..Default::default()
    });

    let upload = task_upload(&mailbox, account).unwrap();
    assert!(upload.ops.iter().all(|op| op.whole.is_some()), "Google is told the whole to-do");
    let asked: RefCell<Vec<(String, String, Value)>> = RefCell::new(Vec::new());
    let fetch = |method: &str, path: &str, body: Option<&str>| {
        let body: Value = body.map(|b| serde_json::from_str(b).unwrap()).unwrap_or(Value::Null);
        asked.borrow_mut().push((method.into(), path.into(), body.clone()));
        let answer = match method {
            "GET" => return replay(method, path, None),
            "POST" => json!({"id": "bmV3MQ", "etag": "\"new\"", "title": body["title"]}),
            _ => json!({"etag": "\"changed\""}),
        };
        Ok((200, answer.to_string().into_bytes()))
    };
    let push = |token: &str, upload: &TasksUpload| {
        assert_eq!(token, "access");
        push_tasks_with(&fetch, upload)
    };
    let out = run_google_with(job(&mailbox, account, upload), &renewed, &push, &|_| tasks_with(&replay));
    let uploaded = out.uploaded.unwrap();
    assert!(uploaded.outcomes.iter().all(|o| o.result == TaskResult::Done), "{:?}", uploaded.outcomes);

    let asked = asked.into_inner();
    let to = |path: &str| asked.iter().find(|(m, p, _)| m != "GET" && p == path).map(|(m, _, b)| (m.clone(), b.clone())).unwrap();
    let (method, body) = to("/lists/MDAxTGlzdA/tasks/dDFHcmF2YXI");
    assert_eq!(method, "PATCH");
    assert_eq!(body, json!({"status": "completed", "completed": "2026-09-21T14:13:20.000Z"}));
    assert_eq!(to("/lists/MDAxTGlzdA/tasks/dDJFZGl0YXI").1, json!({"title": "▶ [Studio] [Urgente] Editar vídeo"}));
    // The subtask: its own notes alone, its parent left as it is.
    assert_eq!(to("/lists/MDAxTGlzdA/tasks/YzFNaWNybw").1, json!({"notes": "Shure ou Rode"}));
    // The new one: in the first list, at the top.
    let (method, made) = to("/lists/MDAxTGlzdA/tasks");
    assert_eq!(method, "POST");
    assert_eq!(made["title"], "[Studio] ? Novo roteiro");
    assert!(made.get("parent").is_none());

    let applied = apply_task_outcomes(&mut mailbox, &uploaded).unwrap();
    assert_eq!(applied.skipped, 0);
    let made = mailbox.tasks.values().find(|t| t.title == "Novo roteiro").unwrap();
    assert_eq!((made.origin.id.as_str(), made.list.as_str()), ("bmV3MQ", "MDAxTGlzdA"));
    assert!(task_upload(&mailbox, account).unwrap().ops.is_empty(), "nothing goes twice");
    assert!(upload_kinds(&mailbox, account).is_empty());
}

fn upload_kinds(mailbox: &Mailbox, account: Id) -> Vec<TaskOpKind> {
    task_upload(mailbox, account).unwrap().ops.iter().map(|op| op.kind).collect()
}

#[test]
fn a_refresh_google_no_longer_takes_asks_for_a_new_connection_and_sends_nothing() {
    let (mailbox, account) = mailbox();
    // (A refusal of the sign-in as the network side words it: an empty token is refused before
    // anything is sent.)
    let refused = |_: &OAuth, _: &str| Err(mailcraft_net::google_tasks::tasks("").unwrap_err());
    let out = run_google_with(job(&mailbox, account, TasksUpload::default()), &refused, &|_, _| unreachable!(), &|_| unreachable!());
    assert!(out.sign_in_refused && out.download.is_none() && out.uploaded.is_none());
    // Google's 403 on the lists: the owner is asked to connect again.
    let out =
        run_google_with(job(&mailbox, account, TasksUpload::default()), &renewed, &|_, _| unreachable!(), &|_| Err(NetError::Forbidden("no".into())));
    assert!(out.consent_needed && out.download.is_none());
}
