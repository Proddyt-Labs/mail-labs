// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: Google Tasks, the owner's to-do lists at Google, kept in step both ways as
//! Microsoft To Do is ([`crate::graph`]'s `tasks`): the lists and their tasks read whole each time
//! (capped: [`MAX_TASK_LISTS`], [`MAX_TASKS_PER_LIST`], [`MAX_TASKS`]), and the changes made here
//! sent one by one. The engine's merge (`mailcraft_engine::cmd::tasks_remote`) is the same for
//! both: this module only turns Google's tasks into [`RemoteTask`]s and [`TaskOp`]s into Google's
//! requests.
//!
//! Everything is `https://tasks.googleapis.com/tasks/v1/…` (`users/@me/lists`, `lists/{list}/tasks`)
//! with an access token for `https://www.googleapis.com/auth/tasks`, which the owner gave on
//! Google's page by itself (`crate::oauth::sign_in_google_tasks`), and that token goes nowhere
//! else ([`may_go_to`]). Nothing of a task (its title, its text, its id) is written to a log.
//!
//! **What Google has and the mailbox doesn't, and the other way round** ([`task_of`],
//! [`task_body`]; one pair, so that a to-do that wasn't changed goes back as it came):
//! - a `[Tag] ` at the start of the title is the category `Tag`; `▶ `, `? ` and `✗ ` are the
//!   categories [`IN_PROGRESS`], [`TO_DO`] and [`CLOSED`]. Going up, the categories become the
//!   start of the title again, in their order.
//! - Google's subtasks: the mailbox has no tasks inside tasks, so a subtask takes its parent's
//!   tags as categories of its own and its notes start with a line `↳ <the parent's title>`.
//!   Going up that line is dropped and those tags are not written into its title; its parent is
//!   never changed, and a to-do made here goes in at the top of its list.
//! - Google keeps no start date, importance or reminder: one set here is not sent, and the next
//!   read takes it back to what Google has. Nor does Google say where in its list a task is kept
//!   here: the order isn't kept in step.
//!
//! What comes back is hostile JSON like everything else from a server: pages are capped, an entry
//! that is no task is skipped, and [`RemoteTask::tidy`] cuts the rest to size.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use mailcraft_model::remote_tasks::{
    MAX_TASK_LISTS, MAX_TASK_OPS, MAX_TASKS, MAX_TASKS_PER_LIST, RemoteTask, RemoteTaskList, TaskOpKind, TaskOutcome, TaskResult, TasksDownload,
    TasksUpload, TasksUploaded,
};
use mailcraft_model::{TaskFields, TaskPatch};
use mailcraft_proto::ProtoError;
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::{Value, json};

use crate::NetError;
use crate::http::{self, Request, Url};

/// Where Google Tasks is.
const API: &str = "https://tasks.googleapis.com";
/// Its host, the one place a Google Tasks token goes.
const API_HOST: &str = "tasks.googleapis.com";
/// Where its version 1 is, on that host.
const BASE: &str = "/tasks/v1";
/// How long Google Tasks has to be reached and to answer one request.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The largest answer read: a page of a hundred tasks with their notes.
const MAX_ANSWER: usize = 4 << 20;
/// The longest token shown to it.
const MAX_TOKEN: usize = 16 * 1024;
/// How many to a page, and the most pages read of one list.
const PAGE_SIZE: &str = "100";
const MAX_PAGES: usize = 200;
/// The longest title and notes Google keeps (characters).
const MAX_TITLE: usize = 1_024;
const MAX_NOTES: usize = 8_192;
/// Longest id of a list or a task that is put into an address, and longest page token followed.
const MAX_ID: usize = 1_024;
/// Most tags and markers read off the start of one title.
const MAX_TAGS: usize = 25;
/// The longest tag taken for a category (the mailbox's longest category name).
const MAX_TAG: usize = mailcraft_model::MAX_NAME;

/// The category a title that starts with `▶ ` has: under way.
pub const IN_PROGRESS: &str = mailcraft_model::STATUS_IN_PROGRESS;
/// The category a title that starts with `? ` has: still to do.
pub const TO_DO: &str = mailcraft_model::STATUS_TO_DO;
/// The category a title that starts with `✗ ` has: closed.
pub const CLOSED: &str = mailcraft_model::STATUS_CLOSED;
/// Each marker at the start of a title and the category it stands for.
const MARKERS: [(&str, &str); 3] = [("▶ ", IN_PROGRESS), ("? ", TO_DO), ("✗ ", CLOSED)];
/// The first line of a subtask's notes starts with this, then its parent's title.
pub const PARENT_LINE: &str = "↳ ";

/// One request to Google Tasks: the method, the path under `/tasks/v1` with its query, and the
/// JSON body; the status and the body of the answer. [`tasks`] and [`push_tasks`] go to Google
/// with it; a test hands in its own.
pub type Fetch<'a> = dyn Fn(&str, &str, Option<&str>) -> Result<(u16, Vec<u8>), NetError> + 'a;

/// The owner's to-dos, as Google Tasks has them.
pub fn tasks(token: &str) -> Result<TasksDownload, NetError> {
    let fetch = client(token)?;
    tasks_with(&fetch)
}

/// Tell Google Tasks what was done here to the owner's to-dos, one change after the other; what it
/// made of each comes back ([`TaskOutcome`]), in the words of Microsoft To Do's
/// (`graph::push_tasks`): 2xx done, 404 and 410 gone, 403 denied (the rest left untried), 409,
/// 412, 429 and 5xx failed for now (the rest left untried), 401 stops with the sign-in refused,
/// anything else refused for good.
pub fn push_tasks(token: &str, upload: &TasksUpload) -> TasksUploaded {
    match client(token) {
        Ok(fetch) => push_tasks_with(&fetch, upload),
        Err(_) => TasksUploaded { account: upload.account, email: upload.email.clone(), token_refused: true, ..Default::default() },
    }
}

/// The requests of [`tasks`] and [`push_tasks`], to Google with `token`.
fn client(token: &str) -> Result<impl Fn(&str, &str, Option<&str>) -> Result<(u16, Vec<u8>), NetError> + '_, NetError> {
    // A token that isn't one is not sent anywhere; it is also what a header can't carry.
    if token.is_empty() || token.len() > MAX_TOKEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Proto(ProtoError::Auth("there is no Google sign-in for the tasks".into())));
    }
    let base = Url::parse(API)?;
    may_go_to(&base)?;
    Ok(move |method: &str, path: &str, body: Option<&str>| {
        let url = Url { path: format!("{BASE}{path}"), ..base.clone() };
        let bearer = format!("Bearer {token}");
        let with_body = [("Authorization", bearer.as_str()), ("Accept", "application/json"), ("Content-Type", "application/json")];
        let headers = if body.is_some() { &with_body[..] } else { &with_body[..2] };
        let request = Request { method, headers, body: body.map(str::as_bytes), patience: TIMEOUT, max_body: MAX_ANSWER };
        let reply = crate::kept::request(&url, &request, method == "GET").map_err(|e| e.without(token))?;
        Ok((reply.status, reply.body))
    })
}

/// Whether a Google Tasks token may go to `url`: Google Tasks' own host, over TLS, on its usual
/// port, and no other place.
pub(crate) fn may_go_to(url: &Url) -> Result<(), NetError> {
    let host = url.host.trim_end_matches('.');
    if url.secure && url.port == 0 && host.eq_ignore_ascii_case(API_HOST) {
        return Ok(());
    }
    Err(NetError::Config("a Google Tasks sign-in is only used with Google Tasks' own server".into()))
}

/// What a read that didn't answer 200 means, for the host to act on.
fn read_error(status: u16) -> NetError {
    match status {
        401 => NetError::Proto(ProtoError::Auth("Google Tasks didn't take the sign-in".into())),
        403 => NetError::Forbidden("this account may not use Google Tasks".into()),
        429 | 500..=599 => NetError::Busy { text: "Google Tasks is busy; try again later".into(), retry_after: None },
        _ => NetError::Proto(ProtoError::Rejected(format!("Google Tasks answered with status {status}").into())),
    }
}

/// One page of a list: the entries in `items`, and the token of the next page, if any.
fn page(fetch: &Fetch<'_>, path: &str) -> Result<(Vec<Value>, Option<String>), NetError> {
    let (status, body) = fetch("GET", path, None)?;
    if status != 200 {
        return Err(read_error(status));
    }
    let json: Value =
        serde_json::from_slice(&body).map_err(|_| NetError::Proto(ProtoError::Protocol("Google Tasks' answer made no sense".into())))?;
    let items = json.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let next = json.get("nextPageToken").and_then(Value::as_str).filter(|t| !t.is_empty() && t.len() <= MAX_ID).map(str::to_string);
    Ok((items, next))
}

/// Every entry of a paged list at `path` (with its query), at most `most`; and whether that was
/// all of it.
fn whole_list(fetch: &Fetch<'_>, path: &str, most: usize) -> Result<(Vec<Value>, bool), NetError> {
    let mut items: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let at = match &next {
            Some(token) => format!("{path}&{}", http::form(&[("pageToken", token)])),
            None => path.to_string(),
        };
        let (got, more) = page(fetch, &at)?;
        for item in got {
            if items.len() >= most {
                return Ok((items, false));
            }
            items.push(item);
        }
        match more {
            Some(token) => next = Some(token),
            None => return Ok((items, true)),
        }
    }
    Ok((items, false))
}

/// [`tasks`] through `fetch`.
pub fn tasks_with(fetch: &Fetch<'_>) -> Result<TasksDownload, NetError> {
    let mut download = TasksDownload::default();
    let lists_path = format!("/users/@me/lists?{}", http::form(&[("maxResults", PAGE_SIZE)]));
    let (lists, whole) = whole_list(fetch, &lists_path, MAX_TASK_LISTS)?;
    download.complete = whole;
    let mut ids: Vec<String> = Vec::new();
    for list in &lists {
        let id = text(list, "id");
        if valid_id(id) && !ids.iter().any(|have| have == id) {
            ids.push(id.to_string());
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    for list in &ids {
        let room = MAX_TASKS.saturating_sub(download.tasks.len());
        if room == 0 {
            download.lists.push(RemoteTaskList { id: list.clone(), complete: false, shared: false });
            continue;
        }
        let query = http::form(&[("maxResults", PAGE_SIZE), ("showCompleted", "true"), ("showHidden", "true"), ("showDeleted", "false")]);
        let path = format!("/lists/{}/tasks?{query}", http::encode(list));
        let (items, whole) = match whole_list(fetch, &path, MAX_TASKS_PER_LIST.min(room)) {
            Ok(read) => read,
            // A list that went while it was being read is not there any more: the next run knows.
            Err(NetError::Proto(ProtoError::Rejected(_))) => {
                download.lists.push(RemoteTaskList { id: list.clone(), complete: false, shared: false });
                continue;
            }
            Err(e) => return Err(e),
        };
        // (A subtask may come before its parent: the titles are known first.)
        let titles: HashMap<&str, &str> = items.iter().map(|item| (text(item, "id"), text(item, "title"))).collect();
        let mut unread = 0usize;
        for item in &items {
            // (A parent that isn't among them, or has no title, names nothing.)
            let parent = Some(text(item, "parent")).filter(|p| !p.is_empty()).and_then(|p| titles.get(p).copied()).filter(|t| !t.trim().is_empty());
            match task_of(item, list, parent) {
                Some(task) if seen.insert(task.id.clone()) => download.tasks.push(task),
                Some(_) => {}
                None if item.get("deleted").and_then(Value::as_bool) == Some(true) => {}
                None => unread += 1,
            }
        }
        download.lists.push(RemoteTaskList { id: list.clone(), complete: whole && unread == 0, shared: false });
    }
    log::debug!("google tasks: {} of them in {} lists, complete: {}", download.tasks.len(), download.lists.len(), download.complete);
    Ok(download)
}

/// An id from Google (or the mailbox file) that is put in an address: none that is empty, too
/// long, has spaces or control characters, or is `.` or `..`.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !matches!(id, "." | "..") && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The text of `name` in `value`, or nothing.
fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

/// The category a marker at the start of a title stands for, and what follows it.
fn marker(title: &str) -> Option<(&'static str, &str)> {
    MARKERS.iter().find_map(|(mark, category)| title.strip_prefix(mark).map(|rest| (*category, rest)))
}

/// Whether `name` can be written as a `[Tag] ` and read back as itself.
fn taggable(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= MAX_TAG && name == name.trim() && !name.chars().any(|c| c.is_control() || c == '[' || c == ']')
}

/// A title as Google has it, read as the categories at its start (markers and `[Tag] `s, in their
/// order) and the rest. A `[Tag]` counts only when a space follows it, so that whatever is read
/// is written back the same ([`title_of`]).
pub fn split_title(raw: &str) -> (Vec<String>, String) {
    let mut categories: Vec<String> = Vec::new();
    let mut rest = raw;
    for _ in 0..MAX_TAGS {
        if let Some((category, after)) = marker(rest) {
            categories.push(category.to_string());
            rest = after;
            continue;
        }
        let tag = rest.strip_prefix('[').and_then(|inside| inside.split_once("] ")).filter(|(name, _)| taggable(name));
        match tag {
            Some((name, after)) => {
                categories.push(name.to_string());
                rest = after;
            }
            None => break,
        }
    }
    (categories, rest.to_string())
}

/// The title Google is to have: each category as its marker or its `[Tag] `, in their order, but
/// those the to-do has from its parent (`inherited`) and those that can't be written as a tag,
/// then the title.
pub fn title_of(categories: &[String], title: &str, inherited: &[String]) -> String {
    let mut out = String::new();
    for category in categories {
        if inherited.iter().any(|from| from.eq_ignore_ascii_case(category)) {
            continue;
        }
        if let Some((mark, _)) = MARKERS.iter().find(|(_, name)| name.eq_ignore_ascii_case(category)) {
            out.push_str(mark);
        } else if taggable(category) {
            out.push('[');
            out.push_str(category);
            out.push_str("] ");
        }
    }
    out.push_str(title);
    out.chars().take(MAX_TITLE).collect()
}

/// A subtask's notes as the mailbox has them: the line naming its parent (by its title as Google
/// has it), then its own notes.
pub fn notes_down(notes: &str, parent: Option<&str>) -> String {
    match parent {
        Some(parent) if notes.trim().is_empty() => format!("{PARENT_LINE}{}", parent.trim()),
        Some(parent) => format!("{PARENT_LINE}{}\n{notes}", parent.trim()),
        None => notes.to_string(),
    }
}

/// The notes Google is to have, and the parent's title as the first line names it (a subtask's,
/// [`notes_down`]): that line is not sent.
pub fn notes_up(notes: &str) -> (String, Option<String>) {
    let Some(first) = notes.strip_prefix(PARENT_LINE) else { return (notes.chars().take(MAX_NOTES).collect(), None) };
    let (parent, rest) = first.split_once('\n').unwrap_or((first, ""));
    (rest.chars().take(MAX_NOTES).collect(), Some(parent.to_string()))
}

/// The tags (not the markers) a subtask takes from its parent's title.
fn parent_tags(parent: &str) -> Vec<String> {
    let markers: Vec<&str> = MARKERS.iter().map(|(_, name)| *name).collect();
    split_title(parent).0.into_iter().filter(|c| !markers.contains(&c.as_str())).collect()
}

/// A day as Google writes it (`2026-11-20T00:00:00.000Z`: Google keeps the date alone).
fn day_of(value: &str) -> Option<Date> {
    Date::parse_iso(value.get(..10)?)
}

/// An instant as Google writes it (RFC 3339).
fn instant(value: &str) -> Option<Timestamp> {
    if value.is_empty() || value.len() > 64 { None } else { mailcraft_time::iso_to_timestamp(value, 0) }
}

/// An instant as Google is to have it.
fn rfc3339(at: Timestamp) -> String {
    format!("{}.000Z", DateTime::from_timestamp(at, 0).iso())
}

/// One of Google's tasks as a [`RemoteTask`] of the list `list`; `parent` is its parent's title
/// as Google has it, for a subtask. `None` for an entry that is no task, or a deleted one.
pub fn task_of(item: &Value, list: &str, parent: Option<&str>) -> Option<RemoteTask> {
    let id = text(item, "id");
    if !valid_id(id) || item.get("deleted").and_then(Value::as_bool) == Some(true) || !item.is_object() {
        return None;
    }
    let (mut categories, title) = split_title(text(item, "title"));
    if let Some(parent) = parent {
        for tag in parent_tags(parent) {
            if !categories.iter().any(|have| have.eq_ignore_ascii_case(&tag)) {
                categories.push(tag);
            }
        }
    }
    let completed = if text(item, "status") == "completed" {
        // (A completed task that doesn't say when is completed when it was last changed.)
        Some(instant(text(item, "completed")).or_else(|| instant(text(item, "updated"))).unwrap_or_else(mailcraft_time::now))
    } else {
        None
    };
    let mut task = RemoteTask {
        id: id.to_string(),
        list: list.to_string(),
        change_key: text(item, "etag").to_string(),
        title,
        notes: notes_down(text(item, "notes"), parent),
        due: day_of(text(item, "due")),
        completed,
        categories,
        ..Default::default()
    };
    task.tidy();
    Some(task)
}

/// The body of a creation or a change: what of `patch` Google keeps. `whole` is the to-do as it is
/// here after the change (the engine sends it for Google: a title is written with the categories).
pub fn task_body(patch: &TaskPatch, whole: Option<&TaskFields>) -> Value {
    let fields = whole.cloned().unwrap_or_else(|| TaskFields::default().after(patch));
    let mut body = serde_json::Map::new();
    let (notes, parent) = notes_up(&fields.notes);
    if patch.title.is_some() || patch.categories.is_some() {
        let inherited = parent.as_deref().map(parent_tags).unwrap_or_default();
        body.insert("title".into(), json!(title_of(&fields.categories, &fields.title, &inherited)));
    }
    if patch.notes.is_some() {
        body.insert("notes".into(), json!(notes));
    }
    if patch.due.is_some() {
        body.insert("due".into(), fields.due.map_or(Value::Null, |day| json!(format!("{}T00:00:00.000Z", day.iso()))));
    }
    if patch.completed.is_some() {
        match fields.completed {
            Some(at) => {
                body.insert("status".into(), json!("completed"));
                body.insert("completed".into(), json!(rfc3339(at)));
            }
            None => {
                body.insert("status".into(), json!("needsAction"));
                body.insert("completed".into(), Value::Null);
            }
        }
    }
    Value::Object(body)
}

/// [`push_tasks`] through `fetch`.
pub fn push_tasks_with(fetch: &Fetch<'_>, upload: &TasksUpload) -> TasksUploaded {
    let mut uploaded = TasksUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    let mut first_list: Option<Result<String, NetError>> = None;
    for op in upload.ops.iter().take(MAX_TASK_OPS) {
        let outcome = |result: TaskResult, error: &str| TaskOutcome {
            kind: op.kind,
            task: op.task,
            id: op.id.clone(),
            list: op.list.clone(),
            patch: op.patch.clone(),
            result,
            error: error.into(),
            ..Default::default()
        };
        if op.kind != TaskOpKind::Create && !(valid_id(&op.id) && valid_id(&op.list)) {
            uploaded.outcomes.push(outcome(TaskResult::Refused, "that isn't a to-do the server could have"));
            continue;
        }
        let body = task_body(&op.patch, op.whole.as_ref());
        if op.kind == TaskOpKind::Update && body.as_object().is_none_or(serde_json::Map::is_empty) {
            // Nothing Google keeps was changed (a start date, an importance, a reminder): nothing
            // to send, and the next read says what Google has.
            uploaded.outcomes.push(outcome(TaskResult::Done, ""));
            continue;
        }
        // A to-do made here goes in the list it names, else the owner's first list (Google's
        // default), looked for once; at the top of it, never under another task.
        let list = if op.kind == TaskOpKind::Create && !valid_id(&op.list) {
            match first_list.get_or_insert_with(|| first_list_of(fetch)) {
                Ok(list) => list.clone(),
                Err(e) if e.is_refused_sign_in() => {
                    uploaded.token_refused = true;
                    break;
                }
                Err(NetError::Forbidden(why)) => {
                    uploaded.outcomes.push(outcome(TaskResult::Denied, why));
                    break;
                }
                Err(e) => {
                    uploaded.outcomes.push(outcome(TaskResult::Failed, &e.to_string()));
                    break;
                }
            }
        } else {
            op.list.clone()
        };
        let tasks = format!("/lists/{}/tasks", http::encode(&list));
        let sent = body.to_string();
        let (method, path, body) = match op.kind {
            TaskOpKind::Create => ("POST", tasks, Some(sent.as_str())),
            TaskOpKind::Update => ("PATCH", format!("{tasks}/{}", http::encode(&op.id)), Some(sent.as_str())),
            TaskOpKind::Delete => ("DELETE", format!("{tasks}/{}", http::encode(&op.id)), None),
        };
        let (status, answer) = match fetch(method, &path, body) {
            Ok(reply) => reply,
            Err(e) => {
                uploaded.outcomes.push(outcome(TaskResult::Failed, &e.to_string()));
                break;
            }
        };
        let mut done = outcome(TaskResult::Done, "");
        done.status = status;
        let stop = match status {
            401 => {
                uploaded.token_refused = true;
                break;
            }
            200..=299 => {
                let json = serde_json::from_slice::<Value>(&answer).unwrap_or(Value::Null);
                done.change_key = text(&json, "etag").to_string();
                if op.kind == TaskOpKind::Create {
                    done.id = text(&json, "id").to_string();
                    done.list = list.clone();
                    // Made, and no word of what it is called: not made again (the next download
                    // shows it, and it is taken in as this one); said, not hidden.
                    if !valid_id(&done.id) {
                        done.result = TaskResult::Refused;
                        done.error = "Google Tasks made it and didn't say what it is called".into();
                    }
                }
                false
            }
            404 | 410 => {
                done.result = TaskResult::Gone;
                false
            }
            403 => {
                done.result = TaskResult::Denied;
                done.error = "this account may not change its Google Tasks".into();
                true
            }
            409 | 412 | 429 | 500..=599 => {
                done.result = TaskResult::Failed;
                done.error = "Google Tasks is busy or the to-do changed there meanwhile".into();
                true
            }
            status => {
                done.result = TaskResult::Refused;
                done.error = format!("Google Tasks refused it (status {status})");
                false
            }
        };
        uploaded.outcomes.push(done);
        if stop {
            break;
        }
    }
    for outcome in &mut uploaded.outcomes {
        outcome.tidy();
    }
    uploaded
}

/// The id of the owner's first list, which is Google's default one.
fn first_list_of(fetch: &Fetch<'_>) -> Result<String, NetError> {
    let path = format!("/users/@me/lists?{}", http::form(&[("maxResults", PAGE_SIZE)]));
    let (items, _) = page(fetch, &path)?;
    items
        .iter()
        .map(|list| text(list, "id"))
        .find(|id| valid_id(id))
        .map(str::to_string)
        .ok_or_else(|| NetError::Proto(ProtoError::Protocol("the account has no list of tasks to make one in".into())))
}
