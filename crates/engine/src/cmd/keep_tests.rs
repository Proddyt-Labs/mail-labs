// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the notes kept in step with Google Keep ([`super::keep`]): downloads merged,
//! changes worked out, and what Keep made of them taken in. Keep's side is written out by hand, as
//! `mailcraft-net` brings it.

use mailcraft_model::keep::{KeepDownload, KeepKind, KeepLabel, KeepNode};
use mailcraft_model::{Id, Note};
use serde_json::{Value, json};

use super::keep::{keep_upload, keep_waiting, merge_keep};
use crate::Session;

const NOW: i64 = 1_790_000_000;
const EMAIL: &str = "ada@example.org";

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": EMAIL, "incoming": {"host": "imap.example.org"}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    run(&mut s, "keep.link", json!({"account": account, "linked": true}));
    (s, account)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn top(id: &str, kind: KeepKind, title: &str, updated: i64) -> KeepNode {
    KeepNode {
        id: id.into(),
        server_id: id.into(),
        parent_id: "root".into(),
        kind,
        title: title.into(),
        color: "white".into(),
        updated,
        raw: format!(r#"{{"id":"{id}","serverId":"{id}","baseVersion":"3","sortValue":"100","unknown":"kept"}}"#),
        ..Default::default()
    }
}

fn item(id: &str, parent: &str, text: &str, checked: bool, sort: i64, updated: i64) -> KeepNode {
    KeepNode {
        id: id.into(),
        server_id: id.into(),
        parent_id: parent.into(),
        kind: KeepKind::Item,
        text: text.into(),
        checked,
        sort,
        updated,
        raw: format!(r#"{{"id":"{id}","serverId":"{id}"}}"#),
        ..Default::default()
    }
}

fn download(account: Id, version: &str, full: bool, nodes: Vec<KeepNode>) -> KeepDownload {
    KeepDownload { account, email: EMAIL.into(), version: version.into(), full, nodes, ..Default::default() }
}

fn titled(s: &Session, title: &str) -> Note {
    s.mailbox.notes.values().find(|n| n.title == title).map(|n| (**n).clone()).unwrap_or_else(|| panic!("no note {title}"))
}

/// A text note with a body, and a checklist of two.
fn first_download(account: Id) -> KeepDownload {
    let mut note = top("n1", KeepKind::Note, "Plan", NOW - 1_000);
    note.labels = vec!["tag.w".into()];
    note.pinned = true;
    note.color = "yellow".into();
    let mut list = top("n2", KeepKind::List, "Groceries", NOW - 900);
    list.archived = true;
    let mut d = download(
        account,
        "v1",
        true,
        vec![
            note,
            item("b1", "n1", "write the plan", false, 5, NOW - 1_000),
            list,
            item("i1", "n2", "milk", false, 20_000, NOW - 900),
            item("i2", "n2", "eggs", true, 30_000, NOW - 900),
            // A picture: left alone.
            KeepNode { id: "p1".into(), parent_id: "n1".into(), kind: KeepKind::Other, ..Default::default() },
        ],
    );
    d.labels = Some(vec![KeepLabel { id: "tag.w".into(), name: "Work".into() }]);
    d
}

#[test]
fn a_first_download_makes_the_notes_and_keeps_them_as_keep_has_them() {
    let (mut s, a) = session();
    let merged = merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    assert_eq!((merged.added, merged.updated, merged.removed), (2, 0, 0));
    let plan = titled(&s, "Plan");
    assert_eq!((plan.text.as_str(), plan.color.as_str(), plan.pinned, plan.checklist), ("write the plan", "yellow", true, false));
    assert_eq!(plan.categories, vec!["Work".to_string()]);
    let link = plan.keep.as_ref().unwrap();
    assert_eq!((link.account, link.id.as_str(), link.body.as_ref().map(|b| b.id.as_str())), (a, "n1", Some("b1")));
    assert!(link.raw.contains("unknown"), "what Keep sent is kept as it came");
    let list = titled(&s, "Groceries");
    assert!(list.checklist && list.archived);
    // Largest sort value first.
    assert_eq!(list.items.iter().map(|i| (i.text.as_str(), i.checked)).collect::<Vec<_>>(), vec![("eggs", true), ("milk", false)]);
    // Nothing is waiting: they are as Keep has them. The version and the labels are noted.
    assert_eq!(keep_waiting(&s.mailbox, a), 0);
    assert!(s.mailbox.notes.values().all(|n| !n.keep_dirty()));
    assert_eq!(s.mailbox.keep[&a].version, "v1");
    assert_eq!(s.mailbox.keep[&a].labels, vec![KeepLabel { id: "tag.w".into(), name: "Work".into() }]);
    // Again: nothing changes.
    let again = merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    assert_eq!((again.added, again.removed), (0, 0));
    assert_eq!(s.mailbox.notes.len(), 2 + crate::sample::mailbox(NOW, 0).notes.len());
}

#[test]
fn a_later_download_changes_one_item_or_removes_a_note() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    // Only an item came (milk checked, and a new one): the note's other fields are as they were.
    let later = download(a, "v2", false, vec![item("i1", "n2", "milk", true, 20_000, NOW), item("i3", "n2", "bread", false, 10_000, NOW)]);
    merge_keep(&mut s.mailbox, &later).unwrap();
    let list = titled(&s, "Groceries");
    assert_eq!(list.items.iter().map(|i| (i.text.as_str(), i.checked)).collect::<Vec<_>>(), vec![("eggs", true), ("milk", true), ("bread", false)]);
    assert!(!list.keep_dirty());
    // An item that went, and a note that went (it came without a parent).
    let mut gone_item = item("i2", "n2", "", false, 0, NOW);
    gone_item.removed = true;
    let mut gone_note = top("n1", KeepKind::Note, "", NOW);
    gone_note.removed = true;
    let merged = merge_keep(&mut s.mailbox, &download(a, "v3", false, vec![gone_item, gone_note])).unwrap();
    assert_eq!(merged.removed, 1);
    assert_eq!(titled(&s, "Groceries").items.len(), 2);
    assert!(s.mailbox.notes.values().all(|n| n.title != "Plan"));
    assert_eq!(s.mailbox.keep[&a].version, "v3");
}

#[test]
fn a_note_changed_here_after_keeps_version_stays_and_goes_up() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    let id = titled(&s, "Plan").id;
    // Changed here at NOW; Keep's version of the node is older than that.
    run(&mut s, "notes.update", json!({"id": id, "title": "Plan B", "color": "red"}));
    assert_eq!(keep_waiting(&s.mailbox, a), 1);
    let mut theirs = top("n1", KeepKind::Note, "Their title", NOW - 10);
    theirs.color = "blue".into();
    let merged = merge_keep(&mut s.mailbox, &download(a, "v2", false, vec![theirs])).unwrap();
    assert_eq!((merged.updated, merged.unchanged), (0, 1));
    let plan = (*s.mailbox.notes[&id]).clone();
    assert_eq!((plan.title.as_str(), plan.color.as_str()), ("Plan B", "red"));
    assert!(plan.keep_dirty(), "it still goes up");
    // Keep's node is newer than the change made here: Keep wins.
    let mut newer = top("n1", KeepKind::Note, "Their title", NOW + 500);
    newer.color = "blue".into();
    let merged = merge_keep(&mut s.mailbox, &download(a, "v3", false, vec![newer])).unwrap();
    assert_eq!(merged.updated, 1);
    let plan = (*s.mailbox.notes[&id]).clone();
    assert_eq!((plan.title.as_str(), plan.color.as_str()), ("Their title", "blue"));
    assert!(!plan.keep_dirty());
}

#[test]
fn a_note_made_here_goes_up_with_its_body_and_a_new_label_and_is_then_as_keep_has_it() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    let made = run(
        &mut s,
        "notes.create",
        json!({"title": "Idea", "text": "  a thought  ", "categories": ["Work", "Home"], "pinned": true, "color": "teal"}),
    );
    assert_eq!(made["keep"], json!(a));
    let id = made["id"].as_u64().unwrap();
    let note = (*s.mailbox.notes[&id]).clone();
    assert_eq!(note.text, "a thought");
    let upload = keep_upload(&s.mailbox, a).unwrap();
    assert_eq!((upload.account, upload.email.as_str(), upload.version.as_str()), (a, EMAIL, "v1"));
    assert_eq!(upload.nodes.len(), 2);
    let (n, body) = (&upload.nodes[0], &upload.nodes[1]);
    assert_eq!((n.kind, n.title.as_str(), n.color.as_str(), n.pinned, n.parent_id.as_str()), (KeepKind::Note, "Idea", "teal", true, "root"));
    assert_eq!((body.kind, body.text.as_str(), body.parent_id.as_str()), (KeepKind::Item, "a thought", n.id.as_str()));
    // "Work" is Keep's already; "Home" is made, and the labels go up with it.
    let labels = upload.labels.clone().unwrap();
    assert_eq!(labels.len(), 2);
    let work = labels.iter().find(|l| l.name == "Work").unwrap();
    assert_eq!(work.id, "tag.w");
    let home = labels.iter().find(|l| l.name == "Home").unwrap();
    assert!(home.id.starts_with("tag."));
    assert_eq!(n.labels.len(), 2);
    assert!(n.labels.contains(&"tag.w".to_string()) && n.labels.contains(&home.id));
    // Keep answers with the nodes (and the labels): the note is then as Keep has it, and nothing waits.
    let mut echo = top(&n.id, KeepKind::Note, "Idea", NOW + 1);
    echo.pinned = true;
    echo.color = "teal".into();
    echo.labels = n.labels.clone();
    let mut reply = download(a, "v2", false, vec![echo, item(&body.id, &n.id, "a thought", false, body.sort, NOW + 1)]);
    reply.labels = Some(labels);
    reply.uploaded = upload.nodes.iter().map(|n| n.id.clone()).collect();
    merge_keep(&mut s.mailbox, &reply).unwrap();
    assert_eq!(keep_waiting(&s.mailbox, a), 0);
    let note = (*s.mailbox.notes[&id]).clone();
    assert_eq!((note.text.as_str(), note.categories.len()), ("a thought", 2));
    assert!(note.keep.unwrap().synced.is_some());
    assert!(keep_upload(&s.mailbox, a).unwrap().nodes.is_empty());
}

#[test]
fn a_checklist_edited_here_tells_keep_what_was_taken_out_and_forgets_it_once_told() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    let list = titled(&s, "Groceries");
    let milk = list.items.iter().find(|i| i.text == "milk").unwrap().id.clone();
    // Eggs are taken out, milk is checked, butter is new.
    run(&mut s, "notes.update", json!({"id": list.id, "items": [{"id": milk, "text": "milk", "checked": true}, {"text": "butter"}]}));
    let upload = keep_upload(&s.mailbox, a).unwrap();
    let nodes: Vec<(&str, bool, bool)> = upload.nodes.iter().map(|n| (n.text.as_str(), n.checked, n.deleted)).collect();
    assert_eq!(nodes, vec![("", false, false), ("milk", true, false), ("butter", false, false), ("eggs", true, true)]);
    // Milk keeps what Keep sent of it; the order goes by sort value, the first the largest.
    assert!(upload.nodes[1].raw.contains("serverId"));
    assert!(upload.nodes[1].sort > upload.nodes[2].sort);
    assert!(upload.nodes[3].deleted);
    let mut reply = download(a, "v2", false, vec![]);
    reply.uploaded = upload.nodes.iter().map(|n| n.id.clone()).collect();
    merge_keep(&mut s.mailbox, &reply).unwrap();
    let kept = s.mailbox.notes[&list.id].keep.clone().unwrap();
    assert!(kept.removed.is_empty(), "Keep was told");
}

#[test]
fn a_note_deleted_for_good_is_remembered_until_keep_is_told() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    let id = titled(&s, "Plan").id;
    run(&mut s, "notes.delete", json!({"id": id}));
    assert!(s.mailbox.notes[&id].trashed, "the Trash first");
    let listed = run(&mut s, "notes.list", json!({}));
    assert!(listed["notes"].as_array().unwrap().iter().all(|n| n["id"] != json!(id)));
    let trash = run(&mut s, "notes.list", json!({"state": "trashed"}));
    assert_eq!(trash["notes"].as_array().unwrap().len(), 1);
    run(&mut s, "notes.deleteForever", json!({"id": id}));
    // Hidden, but there until Keep has been told.
    assert!(s.mailbox.notes[&id].keep.as_ref().unwrap().deleted);
    assert!(s.run("notes.get", &json!({"id": id})).is_ok());
    assert!(s.run("notes.update", &json!({"id": id, "title": "x"})).is_err());
    let upload = keep_upload(&s.mailbox, a).unwrap();
    assert_eq!(upload.nodes.len(), 1);
    assert!(upload.nodes[0].deleted && upload.nodes[0].id == "n1");
    let mut reply = download(a, "v2", false, vec![]);
    reply.uploaded = vec!["n1".into()];
    let merged = merge_keep(&mut s.mailbox, &reply).unwrap();
    assert_eq!(merged.removed, 1);
    assert!(!s.mailbox.notes.contains_key(&id));
}

#[test]
fn a_full_read_takes_away_what_keep_no_longer_has_unless_it_was_changed_here() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    let plan = titled(&s, "Plan").id;
    run(&mut s, "notes.update", json!({"id": plan, "title": "Changed here"}));
    // A full read that names neither note.
    let merged = merge_keep(&mut s.mailbox, &download(a, "v9", true, vec![])).unwrap();
    assert_eq!(merged.removed, 1, "the list went");
    assert!(s.mailbox.notes.values().all(|n| n.title != "Groceries"));
    // The one changed here is made again.
    let kept = (*s.mailbox.notes[&plan]).clone();
    assert_eq!(kept.title, "Changed here");
    assert!(kept.keep.unwrap().synced.is_none());
    assert_eq!(keep_waiting(&s.mailbox, a), 1);
}

#[test]
fn a_download_for_another_account_or_one_without_keep_is_refused_whole() {
    let (mut s, a) = session();
    let mut wrong = first_download(a);
    wrong.email = "someone.else@example.org".into();
    let before = s.mailbox.notes.len();
    assert!(merge_keep(&mut s.mailbox, &wrong).is_err());
    assert_eq!(s.mailbox.notes.len(), before);
    run(&mut s, "keep.link", json!({"account": a, "linked": false}));
    assert!(merge_keep(&mut s.mailbox, &first_download(a)).is_err());
    assert!(keep_upload(&s.mailbox, a).is_err());
    // Unlinked: the notes stay here and nothing is told to Keep.
    run(&mut s, "keep.link", json!({"account": a, "linked": true}));
    assert!(keep_upload(&s.mailbox, a).is_ok());
}

#[test]
fn a_hostile_download_is_cut_to_size_and_never_panics() {
    let (mut s, a) = session();
    let mut note = top("n1", KeepKind::Note, &"t".repeat(5_000), NOW);
    note.labels = vec!["tag.unknown".into(), "bad id".into()];
    let mut it = item("b1", "n1", &format!("x\u{1}y{}", "z".repeat(200_000)), false, i64::MAX, i64::MIN);
    it.raw = "x".repeat(100_000);
    let orphan = item("o1", "no-such-parent", "lost", false, 0, 0);
    let nameless = KeepNode { kind: KeepKind::Note, ..Default::default() };
    merge_keep(&mut s.mailbox, &download(a, "v1", false, vec![note, it, orphan, nameless])).unwrap();
    let n = titled(&s, &"t".repeat(1_000));
    assert!(n.text.chars().count() <= 100_000 && !n.text.chars().any(char::is_control));
    assert!(n.categories.is_empty());
    assert!(n.keep.unwrap().body.unwrap().raw.is_empty());
    assert!(s.mailbox.notes.values().all(|n| n.text != "lost"));
}

#[test]
fn the_commands_take_and_give_no_sign_in() {
    let (mut s, a) = session();
    // A caller that names a token or a cookie means something by it that no command does.
    for name in ["token", "masterToken", "oauth_token", "cookie", "accessToken", "password"] {
        for command in ["notes.create", "notes.update"] {
            let mut params = json!({"id": 1, "title": "x"});
            params[name] = json!("aas_et/NEVER-ECHOED");
            let error = s.run(command, &params).unwrap_err().to_string();
            assert!(!error.contains("NEVER-ECHOED"), "{command} {name}: {error}");
        }
        let mut link = json!({"account": a, "linked": true});
        link[name] = json!("aas_et/NEVER-ECHOED");
        // (`keep.link` reads the two params it knows and nothing else: the value goes nowhere.)
        let answer = run(&mut s, "keep.link", link).to_string();
        assert!(!answer.contains("NEVER-ECHOED"));
    }
    // What the commands answer, for a mailbox full of Keep notes.
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    let id = titled(&s, "Plan").id;
    for (command, params) in [
        ("notes.list", json!({"state": "all"})),
        ("notes.get", json!({"id": id})),
        ("sync.keepChanges", json!({"account": a})),
        ("keep.link", json!({"account": a, "linked": true})),
    ] {
        let said = run(&mut s, command, params).to_string();
        for secret in ["aas_et", "ya29", "oauth2_4", "master", "EncryptedPasswd", "Authorization"] {
            assert!(!said.contains(secret), "{command} says {secret}: {said}");
        }
    }
}

#[test]
fn the_notes_commands_check_their_params() {
    let (mut s, _) = session();
    for (command, params) in [
        ("notes.create", json!({})),
        ("notes.create", json!({"title": "x", "color": "magenta"})),
        ("notes.create", json!({"title": "x", "categories": "Work"})),
        ("notes.create", json!({"items": "milk"})),
        ("notes.create", json!({"title": "x", "account": 424242})),
        ("notes.update", json!({"id": 424242, "title": "x"})),
        ("notes.update", json!({"title": "x"})),
        ("notes.delete", json!({"id": null})),
        ("notes.list", json!({"state": "everything"})),
    ] {
        assert!(s.run(command, &params).is_err(), "{command} {params}");
    }
    // A note kept only here, a plain one and a checklist, with the colour's other name.
    let local = run(&mut s, "notes.create", json!({"title": "Local", "text": "t", "account": null, "color": "default"}));
    assert_eq!(local["keep"], Value::Null);
    let id = local["id"].as_u64().unwrap();
    assert!(s.run("notes.update", &json!({"id": id, "items": [{"text": "a"}]})).is_err(), "a plain note with text is not turned into a list");
    assert!(s.run("notes.update", &json!({"id": id, "text": "new"})).is_ok());
    let list = run(&mut s, "notes.create", json!({"title": "L", "items": [{"text": "a"}, {"text": "b", "checked": true}], "account": null}));
    let got = run(&mut s, "notes.get", json!({"id": list["id"]}));
    assert_eq!((got["checklist"].as_bool(), got["items"].as_u64(), got["itemsDone"].as_u64()), (Some(true), Some(2), Some(1)));
    assert!(s.run("notes.update", &json!({"id": list["id"], "text": "no"})).is_err());
    // Without Keep, forever is forever.
    run(&mut s, "notes.deleteForever", json!({"id": id}));
    assert!(!s.mailbox.notes.contains_key(&id));
}

#[test]
fn notes_are_listed_pinned_first_and_by_label_and_state() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    run(&mut s, "notes.create", json!({"title": "Newest", "text": "n", "account": null}));
    let all = run(&mut s, "notes.list", json!({}));
    let titles: Vec<&str> = all["notes"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap_or("")).collect();
    // Plan is pinned: first. (Groceries is archived: not in the default view.)
    assert_eq!(titles.first(), Some(&"Plan"));
    assert!(!titles.contains(&"Groceries"));
    let archived = run(&mut s, "notes.list", json!({"state": "archived"}));
    assert_eq!(archived["notes"][0]["title"], "Groceries");
    let work = run(&mut s, "notes.list", json!({"label": "work"}));
    assert_eq!(work["total"], 1);
    assert!(all["labels"].as_array().unwrap().iter().any(|l| l == "Work"));
    let found = run(&mut s, "notes.list", json!({"query": "milk", "state": "all"}));
    assert_eq!(found["notes"][0]["title"], "Groceries");
    let view = run(&mut s, "view.notes", json!({}));
    assert_eq!(view["module"], "notes");
}

#[test]
fn removing_an_account_keeps_its_notes_here_and_forgets_its_keep() {
    let (mut s, a) = session();
    merge_keep(&mut s.mailbox, &first_download(a)).unwrap();
    s.run("account.remove", &json!({"id": a})).unwrap();
    assert!(s.mailbox.keep.is_empty());
    let plan = titled(&s, "Plan");
    assert!(plan.keep.is_none());
}
