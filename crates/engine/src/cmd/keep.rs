// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the notes of the owner's Google Keep, kept in step with it both ways
//! (`mailcraft_net::google_keep`), as the to-dos are with Google Tasks
//! ([`super::tasks_remote`]).
//!
//! The engine does no I/O: a host with sockets brings a [`KeepDownload`] (`sync.applyKeep`,
//! [`merge_keep`]) after asking what is to be told to Keep (`sync.keepChanges`, [`keep_upload`]).
//! **No command takes or gives a sign-in**: the master token and the access token are the host's
//! and the system's credential store's; what is here is notes, labels and a version.
//!
//! **What a note remembers.** A note Keep keeps has its [`KeepLink`]: the account, Keep's id for
//! it, the node as Keep sent it, and `synced`, the fields as Keep last had them. What was changed
//! here since is the difference between the two ([`Note::keep_dirty`]); only those notes go up.
//!
//! **Conflicts.** Keep is the source of truth: a note that Keep sends overwrites the note here,
//! unless the note was changed here *after* the time Keep says it was last changed there
//! (`modified` against the node's `updated`): then it stays as it is here and goes up with Keep's
//! version. A note Keep no longer has goes from here too, unless it was changed here since: then
//! it is made again. A note deleted for good here is remembered ([`KeepLink::deleted`]) until
//! Keep has been told.
//!
//! Everything from outside is hostile: ids and text are cut by [`KeepNode::tidy`], counts are
//! capped, and a download for another account is refused whole.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};

use mailcraft_model::keep::{KeepDownload, KeepKind, KeepLabel, KeepNode, KeepUpload, MAX_KEEP_LABELS, MAX_KEEP_UP, MAX_LABEL};
use mailcraft_model::{Id, KeepLink, KeepState, Mailbox, Note, NoteItem};
use mailcraft_time::Timestamp;
use serde_json::{Value, json};

use super::remote::{Merged, owner};
use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session};

const MAX_SHOWN: usize = 200;
/// Keep sorts a list from the largest down, by steps this big.
const SORT_STEP: i64 = 10_000;
const SORT_START: i64 = 5_000_000_000;

/// The colours Keep has, as the mailbox names them (`white` is Keep's default).
pub const COLORS: [&str; 12] = ["white", "red", "orange", "yellow", "green", "teal", "blue", "cerulean", "purple", "pink", "brown", "gray"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("keep.link", "Link Google Keep", "Notes", link_command)
            .params(r#"{"account": number, "linked": bool} — Proddyt Labs fork: whether that account's notes are kept in step with Google Keep. The window links an account once it holds the account's Keep sign-in (in the system's credential store: no command takes or gives it); unlinking keeps the notes here and tells Keep nothing"#),
        CommandSpec::new("sync.keepChanges", "Note Changes to Send", "Send / Receive", keep_changes_command)
            .pure()
            .params(r#"{"account": number} — what that account's Google Keep is to be told of notes made, changed or deleted here: {"account", "email", "version", "nodes": [...], "labels"?: [...]}. A host's, never an agent's"#),
        CommandSpec::new("sync.applyKeep", "Apply Downloaded Notes", "Send / Receive", apply_keep_command)
            .pure()
            .params(r#"{"download": KeepDownload} — what a host brought from Google Keep: {"account", "email"?, "version", "full": bool, "resync": bool, "nodes": [...], "labels"?: [{"id","name"}], "uploaded": [ids], "conflicts": number}; answers how many notes were added, updated and removed"#),
    ]
}

// ---------------------------------------------------------------- text from outside or from a caller

/// One line of text: line breaks and control characters read as spaces, at most `max` characters.
pub fn clean_line(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let short: String = flat.trim().chars().take(max).collect();
    short.trim_end().to_string()
}

/// Text of several lines: its line ends are `\n`, it has no other control characters, and it is at
/// most `max` characters long. (The same cut as the download's, so that a note kept here and sent
/// is the note that comes back.)
pub fn clean_text(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).take(max).collect();
    clean.trim().to_string()
}

/// A label name Keep can keep.
pub fn clean_label(name: &str) -> String {
    clean_line(name, MAX_LABEL)
}

fn now() -> Timestamp {
    mailcraft_time::now()
}

/// A word Keep's protocol uses for a node made here, as the Android app makes one:
/// `<ms in hex>.<16 hex digits>`.
pub fn new_node_id() -> String {
    format!("{:x}.{:016x}", now().saturating_mul(1000), RandomState::new().hash_one(now()))
}

/// A label id for a name that has none yet, as the app makes them: `tag.<12 chars>.<ms in hex>`.
fn new_label_id(name: &str) -> String {
    let hash = RandomState::new().hash_one(name);
    let chars: String = (0..12).map(|i| char::from_digit(((hash >> (i * 4 % 60)) & 0xf) as u32, 16).unwrap_or('0')).collect();
    format!("tag.{chars}.{:x}", now().saturating_mul(1000))
}

/// The sort values for `count` items, the first the largest.
pub fn sorts(count: usize) -> Vec<i64> {
    (0..count).map(|i| SORT_START - SORT_STEP * i64::try_from(i).unwrap_or(0)).collect()
}

// ---------------------------------------------------------------- what is known of the accounts

/// Proddyt Labs fork: whether the account's notes are kept in step with Google Keep.
pub fn keeps_google_keep(mailbox: &Mailbox, account: Id) -> bool {
    mailbox.account(account).is_some_and(|a| a.keeps_google_keep())
}

/// The accounts whose notes are kept in step now, in the order the owner put the accounts in.
pub fn keep_accounts(mailbox: &Mailbox) -> Vec<Id> {
    mailbox.accounts_in_order().into_iter().map(|a| a.id).filter(|id| keeps_google_keep(mailbox, *id)).collect()
}

/// How many notes wait to be told to `account`'s Keep.
pub fn keep_waiting(mailbox: &Mailbox, account: Id) -> usize {
    mailbox.notes.values().filter(|n| n.keep.as_ref().is_some_and(|k| k.account == account) && n.keep_dirty()).count()
}

/// The name of a label, by id, as `account`'s Keep has it.
fn label_name<'a>(state: Option<&'a KeepState>, id: &str) -> Option<&'a str> {
    state?.labels.iter().find(|l| l.id == id).map(|l| l.name.as_str())
}

fn raw_value(raw: &str) -> Option<Value> {
    serde_json::from_str::<Value>(raw).ok().filter(Value::is_object)
}

/// The id Keep gave a node, from the node as it sent it.
fn raw_server_id(raw: &str) -> String {
    raw_value(raw).and_then(|v| v.get("serverId").and_then(Value::as_str).map(str::to_string)).unwrap_or_default()
}

fn raw_sort(raw: &str) -> Option<i64> {
    let value = raw_value(raw)?;
    match value.get("sortValue")? {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

fn raw_created(raw: &str) -> Option<Timestamp> {
    let value = raw_value(raw)?;
    let text = value.pointer("/timestamps/created")?.as_str()?;
    mailcraft_time::iso_to_timestamp(text, 0).filter(|t| *t > 0)
}

// ---------------------------------------------------------------- going up

/// What `account`'s Keep is to be told: the notes changed here (at most [`MAX_KEEP_UP`] nodes; the
/// rest go with the next), as the nodes Keep keeps them in.
pub fn keep_upload(mailbox: &Mailbox, account: Id) -> Result<KeepUpload, CmdError> {
    let owner = mailbox.account(account).filter(|a| a.keeps_google_keep()).ok_or_else(|| bad("that account's notes aren't kept with Google Keep"))?;
    let state = mailbox.keep.get(&account);
    let mut labels: Vec<KeepLabel> = state.map(|s| s.labels.clone()).unwrap_or_default();
    let mut labels_new = false;
    let mut nodes: Vec<KeepNode> = Vec::new();
    for note in mailbox.notes.values().filter(|n| n.keep.as_ref().is_some_and(|k| k.account == account) && n.keep_dirty()) {
        let Some(link) = note.keep.as_ref() else { continue };
        let own = note_nodes(note, link, &mut labels, &mut labels_new);
        if nodes.len() + own.len() > MAX_KEEP_UP && !nodes.is_empty() {
            break;
        }
        nodes.extend(own);
    }
    Ok(KeepUpload {
        account,
        email: owner.email.clone(),
        version: state.map(|s| s.version.clone()).unwrap_or_default(),
        nodes,
        labels: labels_new.then_some(labels),
    })
}

/// A label's id by name (any case), making one when the account has none by that name.
fn label_id(labels: &mut Vec<KeepLabel>, labels_new: &mut bool, name: &str) -> Option<String> {
    let name = clean_label(name);
    if name.is_empty() {
        return None;
    }
    if let Some(have) = labels.iter().find(|l| l.name.eq_ignore_ascii_case(&name)) {
        return Some(have.id.clone());
    }
    if labels.len() >= MAX_KEEP_LABELS {
        return None;
    }
    let id = new_label_id(&name);
    labels.push(KeepLabel { id: id.clone(), name });
    *labels_new = true;
    Some(id)
}

fn note_nodes(note: &Note, link: &KeepLink, labels: &mut Vec<KeepLabel>, labels_new: &mut bool) -> Vec<KeepNode> {
    let mut label_ids: Vec<String> = Vec::new();
    for name in &note.categories {
        if let Some(id) = label_id(labels, labels_new, name)
            && !label_ids.contains(&id)
        {
            label_ids.push(id);
        }
    }
    let server_id = raw_server_id(&link.raw);
    let top = KeepNode {
        id: link.id.clone(),
        server_id: server_id.clone(),
        parent_id: "root".into(),
        kind: if note.checklist { KeepKind::List } else { KeepKind::Note },
        // (A plain note's text is its body node's; one Keep gave with the text on the note itself
        // is written back the same way.)
        text: if note.checklist || link.body.is_some() { String::new() } else { note.text.clone() },
        sort: raw_sort(&link.raw).unwrap_or(SORT_START),
        title: note.title.clone(),
        color: note.color.clone(),
        pinned: note.pinned,
        archived: note.archived,
        trashed: note.trashed,
        deleted: link.deleted,
        labels: label_ids,
        raw: link.raw.clone(),
        ..Default::default()
    };
    let mut out = vec![top];
    if link.deleted {
        return out;
    }
    let child = |item: &NoteItem, text: &str, checked: bool, sort: i64| KeepNode {
        id: item.id.clone(),
        parent_id: link.id.clone(),
        parent_server_id: server_id.clone(),
        kind: KeepKind::Item,
        text: text.to_string(),
        checked,
        sort,
        raw: item.raw.clone(),
        server_id: raw_server_id(&item.raw),
        ..Default::default()
    };
    if note.checklist {
        for item in &note.items {
            out.push(child(item, &item.text, item.checked, item.sort));
        }
        for gone in &link.removed {
            let mut node = child(gone, &gone.text, gone.checked, gone.sort);
            node.deleted = true;
            out.push(node);
        }
    } else if let Some(body) = &link.body {
        out.push(child(body, &note.text, false, body.sort));
    }
    out
}

// ---------------------------------------------------------------- coming down

fn note_of_node(account: Id, node: &KeepNode, state: Option<&KeepState>) -> Note {
    let created = raw_created(&node.raw).unwrap_or(node.updated);
    Note {
        title: node.title.clone(),
        checklist: node.kind == KeepKind::List,
        color: node.color.clone(),
        pinned: node.pinned,
        archived: node.archived,
        trashed: node.trashed,
        categories: names_of(state, &node.labels),
        created,
        modified: node.updated,
        keep: Some(KeepLink { account, id: node.id.clone(), raw: node.raw.clone(), server_updated: node.updated, ..Default::default() }),
        ..Default::default()
    }
}

fn names_of(state: Option<&KeepState>, ids: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for id in ids {
        if let Some(name) = label_name(state, id)
            && !names.iter().any(|have| have.eq_ignore_ascii_case(name))
        {
            names.push(name.to_string());
        }
    }
    names
}

/// Put what Keep says of a note into it (its fields, not its items).
fn take_fields(note: &mut Note, node: &KeepNode, state: Option<&KeepState>) {
    note.title = node.title.clone();
    note.checklist = node.kind == KeepKind::List;
    note.color = node.color.clone();
    note.pinned = node.pinned;
    note.archived = node.archived;
    note.trashed = node.trashed;
    note.categories = names_of(state, &node.labels);
}

/// Take in what a host brought from Keep. A download for another account is refused.
pub fn merge_keep(mailbox: &mut Mailbox, download: &KeepDownload) -> Result<Merged, CmdError> {
    let account = owner(mailbox, download.account, &download.email)?.id;
    if !keeps_google_keep(mailbox, account) {
        return Err(bad("that account's notes aren't kept with Google Keep"));
    }
    let mut d = download.clone();
    d.tidy();
    let mut merged = Merged::default();
    if d.resync {
        // Keep wants everything from the start: the next run asks with no version.
        mailbox.keep.entry(account).or_default().version.clear();
        merged.say("Google Keep asked for the notes to be read again from the start".to_string());
        return Ok(merged);
    }
    {
        let state = mailbox.keep.entry(account).or_default();
        if !d.version.is_empty() {
            state.version = d.version.clone();
        }
        if let Some(labels) = &d.labels {
            state.labels = labels.clone();
        }
    }
    if d.conflicts > 0 {
        merged.say(format!("Google Keep couldn't merge {} of the notes changed here; they go again with its version", d.conflicts));
    }
    let uploaded: HashSet<&str> = d.uploaded.iter().map(String::as_str).collect();
    // What Keep was told and took: items taken out are told, and a note deleted for good is gone.
    let gone_for_good: Vec<Id> = mailbox
        .notes
        .values()
        .filter(|n| n.keep.as_ref().is_some_and(|k| k.account == account && k.deleted && uploaded.contains(k.id.as_str())))
        .map(|n| n.id)
        .collect();
    for id in gone_for_good {
        mailbox.notes.remove(&id);
        merged.removed += 1;
    }
    for note in mailbox.notes.values_mut() {
        if note.keep.as_ref().is_some_and(|k| k.account == account && !k.removed.is_empty()) {
            let note = std::sync::Arc::make_mut(note);
            if let Some(link) = note.keep.as_mut() {
                link.removed.retain(|item| !uploaded.contains(item.id.as_str()));
            }
        }
    }
    let state = mailbox.keep.get(&account).cloned();
    let mut by_link: HashMap<String, Id> = mailbox
        .notes
        .values()
        .filter_map(|n| n.keep.as_ref().filter(|k| k.account == account).map(|k| (k.id.clone(), n.id)))
        .collect();
    // Notes whose Keep fields (and items) were taken from Keep, so that they are as Keep has them.
    let mut taken: HashSet<Id> = HashSet::new();
    // Notes kept as they are here (changed here after Keep's version).
    let mut kept_here: HashSet<Id> = HashSet::new();
    let mut seen_tops: HashSet<String> = HashSet::new();
    for node in d.nodes.iter().filter(|n| matches!(n.kind, KeepKind::Note | KeepKind::List) && !n.id.is_empty()) {
        let local = by_link.get(&node.id).copied();
        if node.removed || node.deleted {
            if let Some(id) = local {
                match mailbox.notes.get(&id).map(|n| n.keep_dirty() && n.keep.as_ref().is_some_and(|k| !k.deleted)) {
                    // Changed here since: made again, with what is here (a change is never lost to a deletion elsewhere).
                    Some(true) => {
                        if let Ok(note) = note_mut(mailbox, id)
                            && let Some(link) = note.keep.as_mut()
                        {
                            link.synced = None;
                            link.raw.clear();
                        }
                    }
                    Some(false) => {
                        mailbox.notes.remove(&id);
                        by_link.remove(&node.id);
                        merged.removed += 1;
                    }
                    None => {}
                }
            }
            continue;
        }
        seen_tops.insert(node.id.clone());
        match local {
            None => {
                let note = note_of_node(account, node, state.as_ref());
                let id = mailbox.add_note(note);
                by_link.insert(node.id.clone(), id);
                taken.insert(id);
                merged.added += 1;
            }
            Some(id) => {
                let newer_here = mailbox.notes.get(&id).is_some_and(|n| n.keep_dirty() && n.modified > node.updated);
                let Ok(note) = note_mut(mailbox, id) else { continue };
                if let Some(link) = note.keep.as_mut() {
                    link.raw = node.raw.clone();
                    link.server_updated = node.updated;
                }
                if newer_here {
                    kept_here.insert(id);
                    merged.unchanged += 1;
                } else {
                    take_fields(note, node, state.as_ref());
                    note.modified = node.updated;
                    taken.insert(id);
                    merged.updated += 1;
                }
            }
        }
    }
    // The items: each is Keep's word unless the note was changed here after it; a note kept as it
    // is here only learns Keep's version of the node.
    let mut item_touched: HashSet<Id> = HashSet::new();
    for node in d.nodes.iter().filter(|n| n.kind == KeepKind::Item && !n.id.is_empty()) {
        let Some(&id) = by_link.get(&node.parent_id) else { continue };
        let newer_here = kept_here.contains(&id) || mailbox.notes.get(&id).is_some_and(|n| n.keep_dirty() && n.modified > node.updated);
        let take = !newer_here;
        let Ok(note) = note_mut(mailbox, id) else { continue };
        let gone = node.removed || node.deleted;
        if note.checklist {
            match note.items.iter().position(|i| i.id == node.id) {
                Some(at) if gone && take => {
                    note.items.remove(at);
                    item_touched.insert(id);
                }
                Some(at) => {
                    if let Some(item) = note.items.get_mut(at) {
                        item.raw = node.raw.clone();
                        if take && !gone {
                            item.text = node.text.clone();
                            item.checked = node.checked;
                            item.sort = node.sort;
                            item_touched.insert(id);
                        }
                    }
                }
                None if take && !gone && note.items.len() < mailcraft_model::keep::MAX_NOTE_ITEMS => {
                    note.items.push(NoteItem { id: node.id.clone(), text: node.text.clone(), checked: node.checked, sort: node.sort, raw: node.raw.clone() });
                    item_touched.insert(id);
                }
                None => {}
            }
        } else if let Some(link) = note.keep.as_mut() {
            // A plain note's body is its one item.
            if gone {
                if link.body.as_ref().is_some_and(|b| b.id == node.id) {
                    link.body = None;
                    if take {
                        note.text.clear();
                        item_touched.insert(id);
                    }
                }
            } else if link.body.as_ref().is_none_or(|b| b.id == node.id) {
                link.body = Some(NoteItem { id: node.id.clone(), text: node.text.clone(), checked: false, sort: node.sort, raw: node.raw.clone() });
                if take {
                    note.text = node.text.clone();
                    item_touched.insert(id);
                }
            }
        }
    }
    for id in taken.iter().chain(item_touched.iter()).copied().collect::<HashSet<Id>>() {
        let Ok(note) = note_mut(mailbox, id) else { continue };
        if note.checklist {
            // Keep lists from the largest sort value down.
            note.items.sort_by_key(|item| std::cmp::Reverse(item.sort));
        } else if taken.contains(&id) && note.keep.as_ref().is_some_and(|k| k.body.is_none()) {
            // (A note with its text on the node itself, as the older clients made them.)
            if let Some(text) = d.nodes.iter().find(|n| by_link.get(&n.id) == Some(&id) && n.kind == KeepKind::Note).map(|n| n.text.clone()) {
                note.text = text;
            }
        }
        let fields = note.keep_fields();
        let whole = taken.contains(&id);
        if let Some(link) = note.keep.as_mut() {
            link.synced = Some(match link.synced.take() {
                // Only the items came: what was changed beside them here stays changed.
                Some(mut earlier) if !whole => {
                    earlier.text = fields.text;
                    earlier.items = fields.items;
                    earlier
                }
                _ => fields,
            });
        }
    }
    // A full read says which notes Keep has: one that was there and isn't is gone (unless changed here since).
    if d.full {
        let stale: Vec<Id> = mailbox
            .notes
            .values()
            .filter(|n| n.keep.as_ref().is_some_and(|k| k.account == account && k.synced.is_some() && !seen_tops.contains(&k.id)))
            .map(|n| n.id)
            .collect();
        for id in stale {
            if mailbox.notes.get(&id).is_some_and(|n| n.keep_dirty()) {
                if let Ok(note) = note_mut(mailbox, id)
                    && let Some(link) = note.keep.as_mut()
                {
                    link.synced = None;
                    link.raw.clear();
                }
            } else {
                mailbox.notes.remove(&id);
                merged.removed += 1;
            }
        }
    }
    Ok(merged)
}

fn note_mut(mailbox: &mut Mailbox, id: Id) -> Result<&mut Note, CmdError> {
    mailbox.notes.get_mut(&id).map(std::sync::Arc::make_mut).ok_or_else(|| fail("no such note"))
}

/// Take in the notes a host downloaded ([`merge_keep`]). Not an undo step, as `sync.applyTasks`.
pub fn apply_keep(s: &mut Session, download: &KeepDownload) -> Result<Merged, CmdError> {
    let merged = merge_keep(&mut s.mailbox, download)?;
    // What Keep was told can't be undone there: going back to a mailbox from before would undo it here only.
    if !download.uploaded.is_empty() {
        s.forget_history();
    }
    s.mailbox_changed();
    Ok(merged)
}

fn download_of(v: &Value) -> Result<KeepDownload, CmdError> {
    let value = v.get("download").cloned().ok_or_else(|| bad("`download` is required"))?;
    serde_json::from_value(value)
        .map_err(|e| bad(format!("`download`: {}", e.to_string().chars().filter(|c| !c.is_control()).take(MAX_SHOWN).collect::<String>())))
}

fn apply_keep_command(s: &mut Session, v: &Value) -> CmdResult {
    let download = download_of(v)?;
    let merged = apply_keep(s, &download)?;
    Ok(json!({
        "added": merged.added, "updated": merged.updated, "removed": merged.removed, "unchanged": merged.unchanged,
        "full": download.full, "messages": merged.messages,
    }))
}

fn keep_changes_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = crate::p::req_id(v, "account")?;
    let upload = keep_upload(&s.mailbox, account)?;
    serde_json::to_value(upload).map_err(|e| fail(e.to_string()))
}

/// Link or unlink an account's notes to Google Keep (`keep.link`).
fn link_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = crate::p::req_id(v, "account")?;
    let linked = v.get("linked").and_then(Value::as_bool).ok_or_else(|| bad("`linked` is required (true or false)"))?;
    let a = s.mailbox.account(account).ok_or_else(|| bad("no such account"))?;
    if linked && a.is_shared() {
        return Err(bad("a shared mailbox's notes can't go to Google Keep"));
    }
    if a.google_keep != linked {
        s.mailbox.account_mut(account).map_err(|e| fail(e.to_string()))?.google_keep = linked;
        s.mailbox_changed();
    }
    Ok(json!({"account": account, "linked": linked}))
}
