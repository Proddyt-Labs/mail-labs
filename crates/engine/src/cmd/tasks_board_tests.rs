// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Tasks board's columns, `tasks.setStatus` and `view.taskLayout`.

use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TaskOpKind, TasksDownload};
use mailcraft_model::{Id, Task};
use serde_json::{Value, json};

use super::tasks::BoardColumn;
use super::tasks_remote::{merge_tasks, task_upload};
use crate::Session;
use crate::agent::{Access, access};

const NOW: i64 = 1_790_000_000;

fn session() -> Session {
    Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
}

fn task(s: &mut Session, title: &str, categories: &[&str]) -> Id {
    let made = s.run("tasks.new", &json!({"title": title, "categories": categories, "account": 0})).unwrap();
    made["id"].as_u64().unwrap()
}

fn set(s: &mut Session, id: Id, status: &str) -> Value {
    s.run("tasks.setStatus", &json!({"id": id, "status": status})).unwrap_or_else(|e| panic!("{status}: {e}"))
}

fn of(s: &Session, id: Id) -> Task {
    s.mailbox.task(id).cloned().unwrap()
}

#[test]
fn a_task_stands_in_the_column_of_its_status() {
    let column = |categories: &[&str], complete: bool| {
        let t = Task { categories: categories.iter().map(|c| c.to_string()).collect(), completed: complete.then_some(NOW), ..Default::default() };
        BoardColumn::of(&t)
    };
    assert_eq!(column(&[], false), BoardColumn::ToDo);
    assert_eq!(column(&["GNT"], false), BoardColumn::ToDo);
    assert_eq!(column(&["GNT", "em andamento"], false), BoardColumn::InProgress);
    assert_eq!(column(&["Fechada"], false), BoardColumn::Closed);
    assert_eq!(column(&["Fechada"], true), BoardColumn::Done);
    // The first status that counts.
    assert_eq!(column(&["Fechada", "A fazer"], false), BoardColumn::Closed);
    for c in BoardColumn::ALL {
        assert_eq!(BoardColumn::parse(c.code()), Some(c));
    }
}

#[test]
fn set_status_replaces_the_status_and_keeps_the_other_categories() {
    let mut s = session();
    let id = task(&mut s, "Prova de GNT", &["GNT", "A fazer"]);
    let said = set(&mut s, id, "inProgress");
    assert_eq!(said["status"], "inProgress");
    assert_eq!(of(&s, id).categories, ["Em andamento", "GNT"]);
    set(&mut s, id, "closed");
    assert_eq!(of(&s, id).categories, ["Fechada", "GNT"]);
    assert_eq!(BoardColumn::of(&of(&s, id)), BoardColumn::Closed);
    // An agent's undo takes it back, as for any edit.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(of(&s, id).categories, ["Em andamento", "GNT"]);
}

#[test]
fn done_completes_and_leaving_done_reopens() {
    let mut s = session();
    let id = task(&mut s, "Entrega AP1", &["AP1", "Em andamento"]);
    let said = set(&mut s, id, "done");
    assert_eq!(said["complete"], true);
    let done = of(&s, id);
    assert!(done.is_complete() && done.categories == ["AP1", "Em andamento"]);
    assert_eq!(BoardColumn::of(&done), BoardColumn::Done);
    // Done twice keeps when it was done.
    set(&mut s, id, "done");
    assert_eq!(of(&s, id).completed, done.completed);
    set(&mut s, id, "toDo");
    let back = of(&s, id);
    assert!(!back.is_complete());
    assert_eq!(back.categories, ["A fazer", "AP1"]);
}

#[test]
fn what_is_wrong_is_refused_and_changes_nothing() {
    let mut s = session();
    let id = task(&mut s, "x", &[]);
    assert!(s.run("tasks.setStatus", &json!({"id": id, "status": "later"})).is_err());
    assert!(s.run("tasks.setStatus", &json!({"id": id})).is_err());
    assert!(s.run("tasks.setStatus", &json!({"id": 999_999, "status": "done"})).is_err());
    assert!(of(&s, id).categories.is_empty() && !of(&s, id).is_complete());
}

#[test]
fn a_status_set_on_the_board_goes_to_the_server_as_an_edit() {
    let mut s = session();
    let account = s.run("account.add", &json!({"email": "work@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()
        ["id"]
        .as_u64()
        .unwrap();
    let lists = vec![RemoteTaskList { id: "L1".into(), complete: true, shared: false }];
    let tasks = vec![RemoteTask { id: "r1".into(), list: "L1".into(), title: "Report".into(), categories: vec!["GNT".into()], ..Default::default() }];
    merge_tasks(&mut s.mailbox, &TasksDownload { account, email: "work@example.org".into(), complete: true, lists, tasks }).unwrap();
    let id = s.mailbox.tasks.values().find(|t| t.title == "Report").map(|t| t.id).unwrap();
    assert!(task_upload(&s.mailbox, account).unwrap().ops.is_empty());
    set(&mut s, id, "inProgress");
    set(&mut s, id, "done");
    let ops = task_upload(&s.mailbox, account).unwrap().ops;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, TaskOpKind::Update);
    assert_eq!(ops[0].patch.categories.as_deref(), Some(&["Em andamento".to_string(), "GNT".to_string()][..]));
    assert!(ops[0].patch.completed.is_some_and(|c| c.is_some()));
}

#[test]
fn the_layout_is_the_list_or_the_board() {
    let mut s = session();
    assert!(!s.view.task_board);
    assert_eq!(s.run("view.taskLayout", &json!({"value": "board"})).unwrap()["value"], "board");
    assert!(s.view.task_board);
    assert!(s.run("view.taskLayout", &json!({"value": "kanban"})).is_err());
    s.run("view.taskLayout", &json!({"value": "list"})).unwrap();
    assert!(!s.view.task_board);
    assert_eq!(access("view.taskLayout"), Some(Access::Safe));
    assert_eq!(access("tasks.setStatus"), access("tasks.categorize"));
}
