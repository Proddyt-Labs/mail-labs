// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: Google Keep ([`crate::google_keep`]), against answers made in the shape the
//! Android app's protocol gives them, and a stand-in for the requests. Nothing here goes to the
//! network, and every note, id and token is made up.

use std::cell::RefCell;

use mailcraft_model::keep::{KeepKind, KeepLabel, KeepNode, KeepUpload};
use serde_json::{Value, json};

use crate::NetError;
use crate::google_keep::{label_json, may_go_to, node_json, node_of, sync_with};
use crate::http::Url;

const NOW: i64 = 1_800_000_000;

fn note(id: &str, title: &str, updated: &str) -> Value {
    json!({
        "id": id, "serverId": id, "kind": "notes#node", "type": "NOTE", "parentId": "root", "sortValue": "1000",
        "baseVersion": "7", "text": "", "title": title, "color": "YELLOW", "isPinned": true, "isArchived": false,
        "labelIds": [{"labelId": "tag.work", "deleted": "1970-01-01T00:00:00.000Z"}, {"labelId": "tag.gone", "deleted": "2026-01-01T00:00:00.000Z"}],
        "timestamps": {"kind": "notes#timestamps", "created": "2026-01-01T10:00:00.000Z", "updated": updated, "trashed": "1970-01-01T00:00:00.000Z"},
        "nodeSettings": {"newListItemPlacement": "BOTTOM", "graveyardState": "COLLAPSED", "checkedListItemsPolicy": "GRAVEYARD"},
        "annotationsGroup": {"kind": "notes#annotationsGroup", "annotations": [{"webLink": {"url": "https://example.org"}}]},
        "someFieldWeDoNotKnow": {"keep": "me"},
    })
}

fn item(id: &str, parent: &str, text: &str, checked: bool) -> Value {
    json!({
        "id": id, "serverId": id, "kind": "notes#node", "type": "LIST_ITEM", "parentId": parent, "parentServerId": parent,
        "sortValue": 5000, "text": text, "checked": checked,
        "timestamps": {"kind": "notes#timestamps", "created": "2026-01-01T10:00:00.000Z", "updated": "2026-02-01T10:00:00.000Z"},
    })
}

/// A stand-in for Keep: it answers the pages in order, and records each request body.
struct Keep {
    pages: RefCell<Vec<(u16, Value)>>,
    seen: RefCell<Vec<Value>>,
}

impl Keep {
    fn new(pages: Vec<(u16, Value)>) -> Keep {
        Keep { pages: RefCell::new(pages), seen: RefCell::default() }
    }

    fn fetch(&self, body: &str) -> Result<(u16, Vec<u8>), NetError> {
        self.seen.borrow_mut().push(serde_json::from_str(body).unwrap());
        let mut pages = self.pages.borrow_mut();
        if pages.is_empty() {
            return Ok((500, b"{}".to_vec()));
        }
        let (status, page) = pages.remove(0);
        Ok((status, page.to_string().into_bytes()))
    }
}

fn run(keep: &Keep, upload: &KeepUpload) -> Result<mailcraft_model::keep::KeepDownload, NetError> {
    sync_with(&|body| keep.fetch(body), upload, NOW, "s--1--2")
}

#[test]
fn a_first_sync_reads_the_pages_until_keep_says_there_are_no_more() {
    let keep = Keep::new(vec![
        (
            200,
            json!({
                "toVersion": "v1", "truncated": true,
                "userInfo": {"labels": [{"mainId": "tag.work", "name": "Work"}, {"mainId": "tag.x", "name": ""}]},
                "nodes": [note("n1", "First", "2026-02-01T10:00:00.000Z"), item("i1", "n1", "milk", false)],
            }),
        ),
        (200, json!({"toVersion": "v2", "truncated": true, "nodes": [note("n2", "Second", "2026-03-01T10:00:00.000Z")]})),
        (200, json!({"toVersion": "v3", "truncated": false, "nodes": [{"id": "n1", "serverId": "n1", "type": "NOTE"}]})),
    ]);
    let got = run(&keep, &KeepUpload { account: 4, email: "someone@example.org".into(), ..Default::default() }).unwrap();
    assert_eq!((got.version.as_str(), got.full, got.resync), ("v3", true, false));
    assert_eq!(got.account, 4);
    assert_eq!(got.nodes.len(), 4);
    assert_eq!(got.nodes.iter().filter(|n| n.kind == KeepKind::Note).count(), 3);
    // (A node that came without a parent has gone from Keep.)
    assert!(got.nodes[3].removed && got.nodes[3].id == "n1");
    assert_eq!(got.labels, Some(vec![KeepLabel { id: "tag.work".into(), name: "Work".into() }]));
    // The requests: no version the first time, then each page's own, and the Android app's headers.
    let seen = keep.seen.borrow();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].get("targetVersion").is_none());
    assert_eq!(seen[1]["targetVersion"], "v1");
    assert_eq!(seen[2]["targetVersion"], "v2");
    for body in seen.iter() {
        assert_eq!(body["requestHeader"]["clientPlatform"], "ANDROID");
        assert_eq!(body["requestHeader"]["clientSessionId"], "s--1--2");
        assert!(body["requestHeader"]["capabilities"].as_array().unwrap().iter().any(|c| c["type"] == "LB"));
        assert_eq!(body["clientTimestamp"], "2027-01-15T08:00:00.000Z");
    }
}

#[test]
fn a_note_is_read_as_the_mailbox_keeps_it() {
    let n = node_of(&note("n1", "Plan", "2026-02-01T10:00:00.000Z")).unwrap();
    assert_eq!((n.kind, n.parent_id.as_str(), n.title.as_str(), n.color.as_str()), (KeepKind::Note, "root", "Plan", "yellow"));
    assert!(n.pinned && !n.archived && !n.trashed && !n.deleted && !n.removed);
    assert_eq!(n.sort, 1000);
    // (Labels taken off are not labels it has.)
    assert_eq!(n.labels, vec!["tag.work".to_string()]);
    assert_eq!(n.updated, mailcraft_time::iso_to_timestamp("2026-02-01T10:00:00Z", 0).unwrap());
    assert!(n.raw.contains("someFieldWeDoNotKnow"));
    let i = node_of(&item("i1", "n1", "milk", true)).unwrap();
    assert_eq!(
        (i.kind, i.parent_id.as_str(), i.text.as_str(), i.checked, i.sort, i.parent_server_id.as_str()),
        (KeepKind::Item, "n1", "milk", true, 5000, "n1")
    );
    // Colour: Keep's default is white; trash and deletion are times, and the epoch is no.
    let mut raw = note("n3", "", "2026-02-01T10:00:00.000Z");
    raw["color"] = json!("DEFAULT");
    raw["timestamps"]["trashed"] = json!("2026-02-02T10:00:00.000Z");
    let trashed = node_of(&raw).unwrap();
    assert_eq!(trashed.color, "white");
    assert!(trashed.trashed);
    // A picture is a node that is left alone: no raw is kept of it.
    let blob = node_of(&json!({"id": "b1", "type": "BLOB", "parentId": "n1", "blob": {"type": "IMAGE"}})).unwrap();
    assert_eq!(blob.kind, KeepKind::Other);
    assert!(blob.raw.is_empty());
    // Hostile entries are no nodes.
    for odd in [json!(null), json!("x"), json!({}), json!({"id": ""}), json!([1])] {
        assert!(node_of(&odd).is_none(), "{odd}");
    }
}

#[test]
fn what_was_changed_here_goes_up_on_the_first_request_and_keeps_what_it_does_not_know() {
    let mut node = node_of(&note("n1", "Plan", "2026-02-01T10:00:00.000Z")).unwrap();
    node.title = "Plan B".into();
    node.pinned = false;
    node.color = "white".into();
    node.trashed = true;
    node.labels = vec!["tag.new".into()];
    let keep = Keep::new(vec![(200, json!({"toVersion": "v9", "truncated": false, "nodes": [note("n1", "Plan B", "2027-01-15T08:00:00.000Z")]}))]);
    let upload = KeepUpload {
        account: 1,
        email: "someone@example.org".into(),
        version: "v8".into(),
        nodes: vec![node],
        labels: Some(vec![KeepLabel { id: "tag.new".into(), name: "New".into() }]),
    };
    let got = run(&keep, &upload).unwrap();
    let seen = keep.seen.borrow();
    assert_eq!(seen[0]["targetVersion"], "v8");
    let sent = &seen[0]["nodes"][0];
    assert_eq!((sent["id"].as_str(), sent["type"].as_str(), sent["parentId"].as_str()), (Some("n1"), Some("NOTE"), Some("root")));
    assert_eq!((sent["title"].as_str(), sent["color"].as_str(), sent["isPinned"].as_bool()), (Some("Plan B"), Some("DEFAULT"), Some(false)));
    // What the program doesn't know went back as it came: the base version, annotations, the field.
    assert_eq!(sent["baseVersion"], "7");
    assert_eq!(sent["someFieldWeDoNotKnow"]["keep"], "me");
    assert_eq!(sent["annotationsGroup"]["annotations"][0]["webLink"]["url"], "https://example.org");
    // Trashed is a time, the labels are as it has them now (the label it had is told as removed).
    assert_eq!(sent["timestamps"]["trashed"], "2027-01-15T08:00:00.000Z");
    assert_eq!(sent["timestamps"]["created"], "2026-01-01T10:00:00.000Z");
    let labels = sent["labelIds"].as_array().unwrap();
    assert!(labels.iter().any(|l| l["labelId"] == "tag.new" && l["deleted"] == "1970-01-01T00:00:00.000Z"));
    assert!(labels.iter().any(|l| l["labelId"] == "tag.work" && l["deleted"] == "2027-01-15T08:00:00.000Z"));
    assert_eq!(seen[0]["userInfo"]["labels"][0]["mainId"], "tag.new");
    // Keep took it: the id is said to be uploaded, and the version moved on.
    assert_eq!(got.uploaded, vec!["n1".to_string()]);
    assert_eq!((got.version.as_str(), got.full), ("v9", false));
}

#[test]
fn a_new_note_is_sent_in_the_shape_the_android_app_makes_one() {
    let note = KeepNode {
        id: "18c.0000000000000001".into(),
        kind: KeepKind::Note,
        title: "Hello".into(),
        color: "white".into(),
        sort: 42,
        ..Default::default()
    };
    let item = KeepNode {
        id: "18c.0000000000000002".into(),
        parent_id: note.id.clone(),
        kind: KeepKind::Item,
        text: "body text".into(),
        sort: 7,
        ..Default::default()
    };
    let n = node_json(&note, NOW);
    assert_eq!(
        (n["type"].as_str(), n["parentId"].as_str(), n["title"].as_str(), n["color"].as_str()),
        (Some("NOTE"), Some("root"), Some("Hello"), Some("DEFAULT"))
    );
    assert_eq!(n["nodeSettings"]["checkedListItemsPolicy"], "GRAVEYARD");
    assert_eq!(n["annotationsGroup"]["kind"], "notes#annotationsGroup");
    assert!(n.get("serverId").is_none() && n.get("labelIds").is_none());
    let i = node_json(&item, NOW);
    assert_eq!(
        (i["type"].as_str(), i["parentId"].as_str(), i["text"].as_str(), i["checked"].as_bool()),
        (Some("LIST_ITEM"), Some("18c.0000000000000001"), Some("body text"), Some(false))
    );
    assert!(i["parentServerId"].is_null());
    let l = label_json(&KeepLabel { id: "tag.a".into(), name: "A".into() }, NOW);
    assert_eq!((l["mainId"].as_str(), l["name"].as_str()), (Some("tag.a"), Some("A")));
}

#[test]
fn a_node_keep_cannot_merge_is_counted_and_not_told_to_have_gone_up() {
    let keep = Keep::new(vec![(
        200,
        json!({"toVersion": "v2", "truncated": false, "nodes": [{"id": "n1", "mergeConflict": true}, note("n2", "Fine", "2026-02-01T10:00:00.000Z")]}),
    )]);
    let upload = KeepUpload {
        nodes: vec![KeepNode { id: "n1".into(), ..Default::default() }, KeepNode { id: "n2".into(), ..Default::default() }],
        version: "v1".into(),
        ..Default::default()
    };
    let got = run(&keep, &upload).unwrap();
    assert_eq!(got.conflicts, 1);
    assert_eq!(got.uploaded, vec!["n2".to_string()]);
    assert_eq!(got.nodes.len(), 1);
}

#[test]
fn a_forced_resync_and_trouble_are_said_not_worked_around() {
    let resync = Keep::new(vec![(200, json!({"forceFullResync": true}))]);
    let got = run(&resync, &KeepUpload { version: "old".into(), ..Default::default() }).unwrap();
    assert!(got.resync && got.nodes.is_empty());
    for (status, refused) in [(401, true), (403, false), (429, false), (503, false)] {
        let keep = Keep::new(vec![(status, json!({"error": {"message": "secret-looking OAuth ya29.LEAKED"}}))]);
        let error = run(&keep, &KeepUpload::default()).unwrap_err();
        assert_eq!(error.is_refused_sign_in(), refused, "{status}");
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("LEAKED"), "{shown}");
    }
    // An error inside a 200 answer is the same; an answer that is no JSON, or says no version, is no answer.
    let inside = Keep::new(vec![(200, json!({"error": {"code": 401}}))]);
    assert!(run(&inside, &KeepUpload::default()).unwrap_err().is_refused_sign_in());
    let no_version = Keep::new(vec![(200, json!({"nodes": []}))]);
    assert!(run(&no_version, &KeepUpload::default()).is_err());
    let not_json = sync_with(&|_| Ok((200, b"<html>".to_vec())), &KeepUpload::default(), NOW, "s");
    assert!(not_json.is_err());
}

#[test]
fn a_hostile_answer_is_cut_to_size() {
    let long = "x".repeat(5_000);
    let mut raw = note("n1", &long, "2026-02-01T10:00:00.000Z");
    raw["text"] = json!("a\u{1}b\u{0}c");
    let keep =
        Keep::new(vec![(200, json!({"toVersion": "v1", "truncated": false, "nodes": [raw, {"id": "bad id", "type": "NOTE", "parentId": "root"}]}))]);
    let got = run(&keep, &KeepUpload::default()).unwrap();
    assert_eq!(got.nodes[0].title.chars().count(), 1_000);
    assert!(!got.nodes[0].text.chars().any(char::is_control));
    // (An id with a space in it names nothing: it is dropped to nothing by `tidy`.)
    assert!(got.nodes[1].id.is_empty());
}

#[test]
fn the_access_token_goes_to_keeps_own_address_alone() {
    assert!(may_go_to(&Url::parse("https://www.googleapis.com/notes/v1/changes").unwrap()).is_ok());
    for elsewhere in [
        "https://www.googleapis.com/tasks/v1/lists",
        "https://tasks.googleapis.com/notes/v1/changes",
        "https://www.googleapis.com.example.org/notes/v1/changes",
        "http://127.0.0.1/notes/v1/changes",
        "https://www.googleapis.com:8443/notes/v1/changes",
    ] {
        assert!(may_go_to(&Url::parse(elsewhere).unwrap()).is_err(), "{elsewhere}");
    }
    // And a token that isn't one is not sent at all.
    let empty = crate::keep_auth::Secret::new(String::new());
    assert!(crate::google_keep::sync(&empty, &KeepUpload::default()).unwrap_err().is_refused_sign_in());
    let spaced = crate::keep_auth::Secret::new("two words".into());
    assert!(crate::google_keep::sync(&spaced, &KeepUpload::default()).unwrap_err().is_refused_sign_in());
}
