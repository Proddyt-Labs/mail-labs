// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: an account's to-dos linked to Google Tasks (`tasks.linkGoogle`): what is
//! sent there, and that nothing of another account ever is. Every address is invented.

use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TaskOpKind, TasksDownload};
use mailcraft_model::{Id, Origin, Task};
use serde_json::json;

use super::tasks_remote::{google_may_take, keeps_google_tasks, keeps_tasks, merge_tasks, task_upload};
use crate::Session;
use crate::agent::{Access, Class, access};

const NOW: i64 = 1_790_000_000;
const PERSONAL: &str = "owner@gmail.example";
const WORK: &str = "owner@work.example";

/// The sample, with a Gmail account kept by IMAP (an app password) and a Microsoft work account.
fn session() -> (Session, Id, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let gmail = s.run("account.add", &json!({"email": PERSONAL, "incoming": {"host": "imap.gmail.com"}})).unwrap()["id"].as_u64().unwrap();
    let work =
        s.run("account.add", &json!({"email": WORK, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()["id"].as_u64().unwrap();
    (s, gmail, work)
}

fn titles(s: &Session, account: Id) -> Vec<String> {
    task_upload(&s.mailbox, account).unwrap().ops.into_iter().filter(|op| op.kind == TaskOpKind::Create).filter_map(|op| op.patch.title).collect()
}

#[test]
fn an_account_is_linked_by_the_owner_and_keeps_its_contacts_where_they_were() {
    let (mut s, gmail, work) = session();
    assert!(!keeps_tasks(&s.mailbox, gmail));
    assert_eq!(access("tasks.linkGoogle"), Some(Access::Owner(Class::Accounts)));
    assert_eq!(access("ui.connectGoogleTasks"), Some(Access::Owner(Class::Accounts)));
    s.run("tasks.linkGoogle", &json!({"account": gmail, "linked": true})).unwrap();
    assert!(keeps_tasks(&s.mailbox, gmail) && keeps_google_tasks(&s.mailbox, gmail));
    assert!(!s.mailbox.account(gmail).unwrap().keeps_contacts());
    // An account whose to-dos Microsoft keeps has one place for them.
    assert!(s.run("tasks.linkGoogle", &json!({"account": work, "linked": true})).is_err());
    assert!(!keeps_google_tasks(&s.mailbox, work));
    assert!(s.run("tasks.linkGoogle", &json!({"account": gmail})).is_err());
    s.run("tasks.linkGoogle", &json!({"account": gmail, "linked": false})).unwrap();
    assert!(!keeps_tasks(&s.mailbox, gmail));
}

#[test]
fn google_is_told_the_whole_to_do_and_microsoft_only_the_change() {
    let (mut s, gmail, work) = session();
    s.run("tasks.linkGoogle", &json!({"account": gmail, "linked": true})).unwrap();
    for account in [gmail, work] {
        s.mailbox.add_task(Task { title: format!("t{account}"), origin: Origin { account, ..Default::default() }, ..Default::default() });
    }
    let google = task_upload(&s.mailbox, gmail).unwrap().ops;
    assert!(!google.is_empty() && google.iter().all(|op| op.whole.is_some()));
    let microsoft = task_upload(&s.mailbox, work).unwrap().ops;
    assert!(!microsoft.is_empty() && microsoft.iter().all(|op| op.whole.is_none()));
}

#[test]
fn nothing_of_another_account_ever_goes_to_google_tasks() {
    let (mut s, gmail, work) = session();
    s.run("tasks.linkGoogle", &json!({"account": gmail, "linked": true})).unwrap();
    // The work account's Microsoft To Do, brought down.
    let lists = vec![RemoteTaskList { id: "W1".into(), complete: true, shared: false }];
    let tasks = vec![RemoteTask { id: "w-1".into(), list: "W1".into(), title: "Work report".into(), ..Default::default() }];
    merge_tasks(&mut s.mailbox, &TasksDownload { account: work, email: WORK.into(), complete: true, lists, tasks }).unwrap();
    // A to-do made from a message of another account and put in the Gmail account.
    let elsewhere = s.mailbox.messages.values().find(|m| m.account != gmail).map(|m| m.id).unwrap();
    s.mailbox.add_task(Task {
        title: "From a work message".into(),
        message: Some(elsewhere),
        origin: Origin { account: gmail, ..Default::default() },
        ..Default::default()
    });
    // The owner's own, made in the Gmail account.
    let own = s.mailbox.add_task(Task { title: "Personal".into(), origin: Origin { account: gmail, ..Default::default() }, ..Default::default() });

    assert_eq!(titles(&s, gmail), ["Personal"]);
    let upload = task_upload(&s.mailbox, gmail).unwrap();
    assert!(upload.ops.iter().all(|op| op.task == own), "only the account's own to-do: {:?}", upload.ops);
    let work_task = s.mailbox.tasks.values().find(|t| t.title == "Work report").unwrap();
    assert!(!google_may_take(&s.mailbox, work_task, gmail));
    // And a download for Google is taken in for its own account alone.
    let wrong = TasksDownload { account: gmail, email: WORK.into(), complete: true, ..Default::default() };
    assert!(merge_tasks(&mut s.mailbox, &wrong).is_err());
}
