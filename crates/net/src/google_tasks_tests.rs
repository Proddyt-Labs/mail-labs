// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: Google Tasks ([`crate::google_tasks`]), against answers recorded in the shape
//! Google gives them (`testdata/google-tasks.json`) and a stand-in for the requests. Nothing here
//! goes to the network.

use std::cell::RefCell;

use mailcraft_model::remote_tasks::{RemoteTask, TaskOp, TaskOpKind, TaskResult, TasksUpload};
use mailcraft_model::{TaskFields, TaskPatch};
use mailcraft_time::Date;
use serde_json::{Value, json};

use crate::NetError;
use crate::google_tasks::{
    CLOSED, IN_PROGRESS, TO_DO, notes_down, notes_up, push_tasks, push_tasks_with, split_title, task_body, task_of, tasks_with, title_of,
};
use crate::http::Url;

const RECORDED: &str = include_str!("../testdata/google-tasks.json");

/// The recorded answers, by `"METHOD path"`.
fn recorded() -> Value {
    serde_json::from_str(RECORDED).unwrap()
}

/// A stand-in for Google that answers each recorded GET, and 404 for anything else.
fn replay(method: &str, path: &str, _body: Option<&str>) -> Result<(u16, Vec<u8>), NetError> {
    match recorded().get(format!("{method} {path}")) {
        Some(answer) => Ok((200, answer.to_string().into_bytes())),
        None => Ok((404, b"{}".to_vec())),
    }
}

/// Every recorded task, as Google has it.
fn recorded_items() -> Vec<Value> {
    recorded()
        .as_object()
        .unwrap()
        .iter()
        .filter(|(key, _)| key.contains("/tasks?"))
        .flat_map(|(_, page)| page["items"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// The fields the mailbox keeps of a downloaded task (as the engine takes them in).
fn fields_of(task: &RemoteTask) -> TaskFields {
    TaskFields {
        title: task.title.clone(),
        notes: task.notes.clone(),
        start: task.start,
        due: task.due,
        completed: task.completed,
        importance: task.importance,
        categories: task.categories.clone(),
        reminder: task.reminder,
    }
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn tags_and_markers_come_off_the_title_in_their_order() {
    assert_eq!(split_title("[Studio] Gravar"), (strings(&["Studio"]), "Gravar".to_string()));
    assert_eq!(split_title("▶ [Studio] Editar"), (strings(&[IN_PROGRESS, "Studio"]), "Editar".to_string()));
    assert_eq!(split_title("[Studio] ▶ Editar"), (strings(&["Studio", IN_PROGRESS]), "Editar".to_string()));
    assert_eq!(split_title("? Revisar"), (strings(&[TO_DO]), "Revisar".to_string()));
    assert_eq!(split_title("✗ [A] [B] Fim"), (strings(&[CLOSED, "A", "B"]), "Fim".to_string()));
    // Not tags: no space after, an empty or untidy name, a bracket in the middle.
    for plain in ["[Studio]Gravar", "[] x", "[ A ] x", "Gravar [Studio] x", "▶Editar", "[Studio]", "?Revisar", "[a[b] x"] {
        assert_eq!(split_title(plain), (Vec::new(), plain.to_string()), "{plain}");
    }
}

#[test]
fn every_title_read_is_written_back_byte_for_byte() {
    let titles = [
        "[Studio] Gravar episódio",
        "▶ [Studio] Editar vídeo",
        "[Studio] ▶ Editar vídeo",
        "? Revisar contrato",
        "✗ Reunião com a gráfica",
        "[A] [B] ✗ Fim",
        "[Studio] ",
        "Sem nada",
        "[Studio]Gravar",
        "[] x",
        "",
    ];
    for raw in titles {
        let (categories, title) = split_title(raw);
        assert_eq!(title_of(&categories, &title, &[]), raw, "{raw}");
    }
}

#[test]
fn categories_go_up_as_markers_and_tags() {
    let cats = strings(&["em andamento", "Studio", "A fazer"]);
    assert_eq!(title_of(&cats, "X", &[]), "▶ [Studio] ? X");
    // Inherited from the parent: left to the parent's title. A name that can't be a tag is left out.
    assert_eq!(title_of(&strings(&["Own", "Studio"]), "X", &strings(&["studio"])), "[Own] X");
    assert_eq!(title_of(&strings(&["a]b", " pad", "Ok"]), "X", &[]), "[Ok] X");
    // And what goes up comes back the same.
    let (back, title) = split_title(&title_of(&strings(&["Ok", "Fechada"]), "Y", &[]));
    assert_eq!((back, title.as_str()), (strings(&["Ok", CLOSED]), "Y"));
}

#[test]
fn a_subtask_names_its_parent_in_its_notes_and_never_sends_that_line() {
    assert_eq!(notes_down("", Some("[Studio] Pai")), "↳ [Studio] Pai");
    assert_eq!(notes_down("texto", Some("Pai")), "↳ Pai\ntexto");
    assert_eq!(notes_down("texto", None), "texto");
    assert_eq!(notes_up("↳ Pai\ntexto\nmais"), ("texto\nmais".to_string(), Some("Pai".to_string())));
    assert_eq!(notes_up("↳ [Studio] Pai"), (String::new(), Some("[Studio] Pai".to_string())));
    assert_eq!(notes_up("só texto"), ("só texto".to_string(), None));
    for (notes, parent) in [("", "[S] P"), ("a\nb", "P"), ("x", "▶ [S] P")] {
        assert_eq!(notes_up(&notes_down(notes, Some(parent))), (notes.to_string(), Some(parent.to_string())));
    }
}

#[test]
fn a_subtask_takes_its_parents_tags_but_not_its_markers() {
    let child = json!({"id": "c1", "title": "? Comprar", "notes": "loja", "status": "needsAction"});
    let task = task_of(&child, "L1", Some("▶ [Studio] [Casa] Gravar")).unwrap();
    assert_eq!(task.categories, strings(&[TO_DO, "Studio", "Casa"]));
    assert_eq!(task.notes, "↳ ▶ [Studio] [Casa] Gravar\nloja");
    assert_eq!(task.title, "Comprar");
    // Going up: only its own marker is written in its title, the parent line stays out of the notes.
    let fields = fields_of(&task);
    let body = task_body(&fields.whole(), Some(&fields));
    assert_eq!(body["title"], "? Comprar");
    assert_eq!(body["notes"], "loja");
    // A tag it has itself and its parent too is the parent's.
    let both = task_of(&json!({"id": "c2", "title": "[Studio] Y", "status": "needsAction"}), "L1", Some("[Studio] P")).unwrap();
    assert_eq!(both.categories, strings(&["Studio"]));
}

#[test]
fn the_recorded_download_reads_every_list_and_page() {
    let download = tasks_with(&replay).unwrap();
    assert!(download.complete);
    let lists: Vec<(&str, bool)> = download.lists.iter().map(|l| (l.id.as_str(), l.complete)).collect();
    assert_eq!(lists, [("MDAxTGlzdA", true), ("MDJTdHVkaW8", true)]);
    assert_eq!(download.tasks.len(), 5);
    let find = |id: &str| download.tasks.iter().find(|t| t.id == id).unwrap();

    let record = find("dDFHcmF2YXI");
    assert_eq!((record.title.as_str(), record.list.as_str()), ("Gravar episódio", "MDAxTGlzdA"));
    assert_eq!(record.categories, strings(&["Studio"]));
    assert_eq!(record.due, Date::new(2026, 11, 20));
    assert_eq!(record.notes, "Roteiro pronto");
    assert_eq!(record.change_key, "\"LTE3MDAwMDAwMDE\"");
    assert!(record.completed.is_none());

    assert_eq!(find("dDJFZGl0YXI").categories, strings(&[IN_PROGRESS, "Studio"]));

    // The subtask is on the second page, its parent on the first.
    let child = find("YzFNaWNybw");
    assert_eq!(child.title, "Comprar microfone");
    assert_eq!(child.categories, strings(&["Studio"]));
    assert_eq!(child.notes, "↳ [Studio] Gravar episódio");

    let closed = find("dDRDYW5jZWw");
    assert_eq!(closed.categories, strings(&[CLOSED]));
    assert_eq!(closed.completed, mailcraft_time::iso_to_timestamp("2026-11-09T15:30:00Z", 0));

    let other = find("dDVSZXZpc2Fy");
    assert_eq!((other.list.as_str(), other.notes.as_str()), ("MDJTdHVkaW8", "Linha 1\nLinha 2"));
    assert_eq!(other.categories, strings(&[TO_DO]));
}

#[test]
fn an_unchanged_recorded_task_goes_back_as_it_came() {
    let download = tasks_with(&replay).unwrap();
    for item in recorded_items() {
        let task = download.tasks.iter().find(|t| t.id == item["id"].as_str().unwrap()).unwrap();
        let fields = fields_of(task);
        let body = task_body(&fields.whole(), Some(&fields));
        assert_eq!(body["title"], item["title"], "{item}");
        assert_eq!(body["notes"].as_str().unwrap(), item["notes"].as_str().unwrap_or(""), "{item}");
        assert_eq!(body["due"], item.get("due").cloned().unwrap_or(Value::Null), "{item}");
        assert_eq!(body["status"], item["status"], "{item}");
        if item["status"] == "completed" {
            assert_eq!(body["completed"], item["completed"], "{item}");
        }
        // And read again, it is the same to-do.
        let mut again = item.clone();
        for key in ["title", "notes", "due", "status", "completed"] {
            if let Some(value) = body.get(key).filter(|v| !v.is_null()) {
                again[key] = value.clone();
            }
        }
        let parent = item.get("parent").and(Some("[Studio] Gravar episódio"));
        assert_eq!(task_of(&again, &task.list, parent).as_ref(), Some(task), "{item}");
    }
}

#[test]
fn a_change_sends_only_what_google_keeps() {
    let here = TaskFields { title: "Gravar".into(), categories: strings(&["Studio"]), notes: "n".into(), ..Default::default() };
    // Only the state: nothing else goes.
    let patch = TaskPatch { completed: Some(Some(1_763_000_000)), ..Default::default() };
    let body = task_body(&patch, Some(&TaskFields { completed: Some(1_763_000_000), ..here.clone() }));
    assert_eq!(body, json!({"status": "completed", "completed": "2025-11-13T02:13:20.000Z"}));
    let reopened = task_body(&TaskPatch { completed: Some(None), ..Default::default() }, Some(&here));
    assert_eq!(reopened, json!({"status": "needsAction", "completed": null}));
    // A category changed: the title goes whole, with the tags.
    let tagged = task_body(&TaskPatch { categories: Some(strings(&["Studio"])), ..Default::default() }, Some(&here));
    assert_eq!(tagged, json!({"title": "[Studio] Gravar"}));
    // A due date, and none.
    let due = task_body(&TaskPatch { due: Some(Date::new(2026, 12, 1)), ..Default::default() }, None);
    assert_eq!(due, json!({"due": "2026-12-01T00:00:00.000Z"}));
    assert_eq!(task_body(&TaskPatch { due: Some(None), ..Default::default() }, None), json!({"due": null}));
    // What Google doesn't keep is not sent.
    let unkept = TaskPatch { importance: Some(mailcraft_model::Importance::High), start: Some(Date::new(2026, 1, 1)), ..Default::default() };
    assert_eq!(task_body(&unkept, Some(&here)), json!({}));
}

/// A stand-in that notes every request and answers from `answers` in turn.
struct Server {
    asked: RefCell<Vec<(String, String, Option<Value>)>>,
    answers: RefCell<Vec<(u16, Value)>>,
}

impl Server {
    fn new(answers: Vec<(u16, Value)>) -> Server {
        Server { asked: RefCell::new(Vec::new()), answers: RefCell::new(answers) }
    }

    fn fetch(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, Vec<u8>), NetError> {
        self.asked.borrow_mut().push((method.into(), path.into(), body.map(|b| serde_json::from_str(b).unwrap())));
        let mut answers = self.answers.borrow_mut();
        let (status, answer) = if answers.is_empty() { (500, json!({})) } else { answers.remove(0) };
        Ok((status, answer.to_string().into_bytes()))
    }
}

fn upload(ops: Vec<TaskOp>) -> TasksUpload {
    TasksUpload { account: 4, email: "owner@example.org".into(), ops }
}

#[test]
fn a_new_task_goes_to_the_first_list_at_the_top() {
    let fields = TaskFields { title: "Novo".into(), categories: strings(&["Studio", IN_PROGRESS]), notes: "x".into(), ..Default::default() };
    let op = TaskOp { kind: TaskOpKind::Create, task: 9, patch: fields.whole(), whole: Some(fields), ..Default::default() };
    let server = Server::new(vec![
        (200, json!({"items": [{"id": "L1"}, {"id": "L2"}]})),
        (200, json!({"kind": "tasks#task", "id": "NEW1", "etag": "\"e1\"", "title": "[Studio] ▶ Novo"})),
    ]);
    let done = push_tasks_with(&|m: &str, p: &str, b: Option<&str>| server.fetch(m, p, b), &upload(vec![op]));
    let asked = server.asked.borrow();
    assert_eq!(asked[0].0, "GET");
    assert_eq!((asked[1].0.as_str(), asked[1].1.as_str()), ("POST", "/lists/L1/tasks"));
    let body = asked[1].2.clone().unwrap();
    assert_eq!(body["title"], "[Studio] ▶ Novo");
    assert_eq!(body["notes"], "x");
    assert!(body.get("parent").is_none(), "never under another task");
    let outcome = &done.outcomes[0];
    assert_eq!((outcome.result, outcome.id.as_str(), outcome.list.as_str(), outcome.task), (TaskResult::Done, "NEW1", "L1", 9));
    assert_eq!(outcome.change_key, "\"e1\"");
}

#[test]
fn changes_and_deletions_go_to_their_task() {
    let here = TaskFields { title: "T".into(), completed: Some(1_763_000_000), ..Default::default() };
    let change = TaskOp {
        kind: TaskOpKind::Update,
        task: 3,
        id: "T1".into(),
        list: "L1".into(),
        patch: TaskPatch { completed: Some(Some(1_763_000_000)), ..Default::default() },
        whole: Some(here),
    };
    let unkept = TaskOp {
        kind: TaskOpKind::Update,
        task: 4,
        id: "T2".into(),
        list: "L1".into(),
        patch: TaskPatch { importance: Some(mailcraft_model::Importance::High), ..Default::default() },
        whole: Some(TaskFields::default()),
    };
    let delete = TaskOp { kind: TaskOpKind::Delete, id: "T3".into(), list: "L2".into(), ..Default::default() };
    let gone = TaskOp { kind: TaskOpKind::Delete, id: "T4".into(), list: "L2".into(), ..Default::default() };
    let server = Server::new(vec![(200, json!({"id": "T1", "etag": "\"e2\""})), (204, json!(null)), (404, json!({}))]);
    let done = push_tasks_with(&|m: &str, p: &str, b: Option<&str>| server.fetch(m, p, b), &upload(vec![change, unkept, delete, gone]));
    let asked = server.asked.borrow();
    let shown: Vec<(&str, &str)> = asked.iter().map(|(m, p, _)| (m.as_str(), p.as_str())).collect();
    assert_eq!(shown, [("PATCH", "/lists/L1/tasks/T1"), ("DELETE", "/lists/L2/tasks/T3"), ("DELETE", "/lists/L2/tasks/T4")]);
    assert_eq!(asked[0].2, Some(json!({"status": "completed", "completed": "2025-11-13T02:13:20.000Z"})));
    let results: Vec<TaskResult> = done.outcomes.iter().map(|o| o.result).collect();
    assert_eq!(results, [TaskResult::Done, TaskResult::Done, TaskResult::Done, TaskResult::Gone]);
    assert_eq!(done.outcomes[0].change_key, "\"e2\"");
}

#[test]
fn refusals_stop_or_are_noted() {
    let del = |id: &str| TaskOp { kind: TaskOpKind::Delete, id: id.into(), list: "L1".into(), ..Default::default() };
    // 401: nothing more is sent.
    let server = Server::new(vec![(401, json!({}))]);
    let done = push_tasks_with(&|m: &str, p: &str, b: Option<&str>| server.fetch(m, p, b), &upload(vec![del("A"), del("B")]));
    assert!(done.token_refused && done.outcomes.is_empty() && server.asked.borrow().len() == 1);
    // 403: denied, the rest untried.
    let server = Server::new(vec![(403, json!({}))]);
    let done = push_tasks_with(&|m: &str, p: &str, b: Option<&str>| server.fetch(m, p, b), &upload(vec![del("A"), del("B")]));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [TaskResult::Denied]);
    // 429: failed for now, the rest untried. 400: refused for good, the rest goes on.
    let server = Server::new(vec![(400, json!({})), (429, json!({}))]);
    let done = push_tasks_with(&|m: &str, p: &str, b: Option<&str>| server.fetch(m, p, b), &upload(vec![del("A"), del("B"), del("C")]));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [TaskResult::Refused, TaskResult::Failed]);
    // An id that can't go in an address is never sent.
    let server = Server::new(vec![]);
    let done = push_tasks_with(&|m: &str, p: &str, b: Option<&str>| server.fetch(m, p, b), &upload(vec![del("a b")]));
    assert_eq!(done.outcomes[0].result, TaskResult::Refused);
    assert!(server.asked.borrow().is_empty());
    // Made, and no word of its id: not made again.
    let made = TaskFields { title: "N".into(), ..Default::default() };
    let create = TaskOp { kind: TaskOpKind::Create, list: "L1".into(), patch: made.whole(), whole: Some(made), ..Default::default() };
    let server = Server::new(vec![(200, json!({}))]);
    let done = push_tasks_with(&|m: &str, p: &str, b: Option<&str>| server.fetch(m, p, b), &upload(vec![create]));
    assert_eq!(done.outcomes[0].result, TaskResult::Refused);
}

#[test]
fn reads_that_fail_say_how() {
    let status = |code: u16| move |_: &str, _: &str, _: Option<&str>| Ok((code, b"{}".to_vec()));
    assert!(tasks_with(&status(401)).unwrap_err().is_refused_sign_in());
    assert!(matches!(tasks_with(&status(403)), Err(NetError::Forbidden(_))));
    assert!(matches!(tasks_with(&status(503)), Err(NetError::Busy { .. })));
    // Nonsense is no download.
    assert!(tasks_with(&|_: &str, _: &str, _: Option<&str>| Ok((200, b"<html>".to_vec()))).is_err());
}

#[test]
fn hostile_entries_are_skipped_and_cut() {
    let page = json!({"items": [
        {"id": "", "title": "no id"},
        {"id": "a b", "title": "space in id"},
        {"id": "..", "title": "dots"},
        {"id": "ok", "title": "x".repeat(5_000), "status": "needsAction", "due": "nonsense"},
        "not an object",
        {"id": "gone", "title": "d", "deleted": true}
    ]});
    let fetch = move |_: &str, path: &str, _: Option<&str>| {
        let body = if path.starts_with("/users/") { json!({"items": [{"id": "L1"}]}) } else { page.clone() };
        Ok((200, body.to_string().into_bytes()))
    };
    let download = tasks_with(&fetch).unwrap();
    assert_eq!(download.tasks.len(), 1);
    assert!(download.tasks[0].title.chars().count() <= 500 && download.tasks[0].due.is_none());
    // Entries that couldn't be read leave the list not whole: nothing is removed for them.
    assert!(!download.lists[0].complete);
}

#[test]
fn the_token_goes_to_google_tasks_alone() {
    assert!(crate::google_tasks::may_go_to(&Url::parse("https://tasks.googleapis.com").unwrap()).is_ok());
    for elsewhere in ["https://evil.example", "https://tasks.googleapis.com:8443", "https://gmail.googleapis.com", "http://127.0.0.1:9"] {
        assert!(crate::google_tasks::may_go_to(&Url::parse(elsewhere).unwrap()).is_err(), "{elsewhere}");
    }
    // A token that can't be one is not sent at all.
    let refused = push_tasks("bad token\r\n", &upload(vec![]));
    assert!(refused.token_refused);
    assert!(crate::google_tasks::tasks("").unwrap_err().is_refused_sign_in());
}
