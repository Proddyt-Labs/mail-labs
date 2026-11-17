// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: Google Keep, the owner's notes at Google, kept in step both ways through the
//! protocol the Android app speaks (`https://www.googleapis.com/notes/v1/changes`; the sign-in is
//! [`crate::keep_auth`]). It is not a published interface: when Google changes it the sync stops
//! until this module is mended, and nothing is lost (Keep stays the source of truth).
//!
//! One request carries what was changed here (nodes) and asks for what changed there, from the
//! version the notes were last read at (`targetVersion`); the answer says what changed, up to
//! which version, and whether there is more (`truncated`: asked again until there isn't). A note
//! is a node of type `NOTE` or `LIST` under `root`; a checklist's items, and the text of a plain
//! note, are `LIST_ITEM` nodes under it. Pictures, drawings and sounds are `BLOB` nodes: read as
//! [`KeepKind::Other`] and never touched. Reminders are not asked for.
//!
//! **What goes back as it came.** A node that is changed here is sent as Keep gave it with the
//! fields this program knows replaced ([`node_json`]), so what it doesn't know (annotations,
//! sharing, the base version) is not lost.
//!
//! **Secrets.** The access token goes to Keep's own address alone ([`may_go_to`]), in a header,
//! and is not in any error: an answer is never repeated, only one of the fixed sentences here.
//! Nothing of a note (title, text, id) is written to a log.

use std::time::Duration;

use mailcraft_model::keep::{KeepDownload, KeepKind, KeepLabel, KeepNode, KeepUpload, MAX_KEEP_LABELS, MAX_KEEP_NODES};
use mailcraft_proto::ProtoError;
use mailcraft_time::{DateTime, Timestamp};
use serde_json::{Value, json};

use crate::NetError;
use crate::http::{Request, Url};
use crate::keep_auth::Secret;

const API: &str = "https://www.googleapis.com";
const API_HOST: &str = "www.googleapis.com";
const CHANGES: &str = "/notes/v1/changes";
const TIMEOUT: Duration = Duration::from_secs(60);
/// The largest answer read: a page of Keep's changes.
const MAX_ANSWER: usize = 32 << 20;
const MAX_TOKEN: usize = 16 * 1024;
/// The most pages of changes followed in one sync.
const MAX_PAGES: usize = 500;
/// The version of Keep's list of changes is a word from Keep; it is held to a size.
const MAX_VERSION: usize = 512;
/// What Google's own epoch is, for "not trashed", "not deleted".
const EPOCH: &str = "1970-01-01T00:00:00.000Z";

/// One request to Keep: the body (JSON), and the status and body of the answer. [`sync`] goes to
/// Google with it; a test hands in its own.
pub type Fetch<'a> = dyn Fn(&str) -> Result<(u16, Vec<u8>), NetError> + 'a;

/// Whether a Keep access token may go to `url`: Keep's own address, over TLS, on its usual port.
pub(crate) fn may_go_to(url: &Url) -> Result<(), NetError> {
    let host = url.host.trim_end_matches('.');
    if url.secure && url.port == 0 && host.eq_ignore_ascii_case(API_HOST) && url.path.starts_with("/notes/v1/") {
        return Ok(());
    }
    Err(NetError::Config("a Google Keep sign-in is only used with Google Keep's own server".into()))
}

/// Read the owner's Keep notes and tell it what was changed here: [`sync_with`] to Google, with
/// the access token `access` (in a header, never in an error).
pub fn sync(access: &Secret, upload: &KeepUpload) -> Result<KeepDownload, NetError> {
    let fetch = client(access)?;
    sync_with(&fetch, upload, mailcraft_time::now(), &new_session(mailcraft_time::now()))
}

/// A name for this run, as the Android app makes one: `s--<ms>--<ten digits>`.
fn new_session(now: Timestamp) -> String {
    use std::hash::{BuildHasher, RandomState};
    let random = RandomState::new().hash_one(now) % 9_000_000_000 + 1_000_000_000;
    format!("s--{}--{random}", now.saturating_mul(1000))
}

fn client(access: &Secret) -> Result<impl Fn(&str) -> Result<(u16, Vec<u8>), NetError> + '_, NetError> {
    let token = access.expose();
    // A token that isn't one is not sent anywhere; it is also what a header can't carry.
    if token.is_empty() || token.len() > MAX_TOKEN || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Proto(ProtoError::Auth("there is no Google Keep sign-in".into())));
    }
    let mut url = Url::parse(API)?;
    url.path = CHANGES.to_string();
    may_go_to(&url)?;
    Ok(move |body: &str| {
        let bearer = format!("OAuth {token}");
        let headers = [("Authorization", bearer.as_str()), ("Accept", "application/json"), ("Content-Type", "application/json")];
        let request = Request { method: "POST", headers: &headers, body: Some(body.as_bytes()), patience: TIMEOUT, max_body: MAX_ANSWER };
        // A change that is sent twice is the same change, but a connection that waited may have
        // closed: a new one each time (`repeatable: false`).
        let reply = crate::kept::request(&url, &request, false).map_err(|e| e.without(token))?;
        Ok((reply.status, reply.body))
    })
}

/// What a request that didn't answer 200 means, for the host to act on. Never the answer's words.
fn read_error(status: u16) -> NetError {
    match status {
        401 => NetError::Proto(ProtoError::Auth("Google Keep didn't take the sign-in".into())),
        403 => NetError::Forbidden("this account may not use Google Keep this way".into()),
        429 | 500..=599 => NetError::Busy { text: "Google Keep is busy; try again later".into(), retry_after: None },
        _ => NetError::Proto(ProtoError::Rejected(format!("Google Keep answered with status {status}").into())),
    }
}

/// [`sync`] through `fetch`, as of `now`, as session `session`.
pub fn sync_with(fetch: &Fetch<'_>, upload: &KeepUpload, now: Timestamp, session: &str) -> Result<KeepDownload, NetError> {
    let mut download = KeepDownload { account: upload.account, email: upload.email.clone(), full: upload.version.is_empty(), ..Default::default() };
    let mut version = (!upload.version.is_empty()).then(|| upload.version.clone());
    let mut nodes: Vec<Value> = upload.nodes.iter().map(|n| node_json(n, now)).collect();
    let mut labels: Option<Vec<Value>> = upload.labels.as_ref().map(|l| l.iter().map(|l| label_json(l, now)).collect());
    let sent: Vec<String> = upload.nodes.iter().map(|n| n.id.clone()).collect();
    let mut first = true;
    for _ in 0..MAX_PAGES {
        let body = request_body(version.as_deref(), std::mem::take(&mut nodes), labels.take(), now, session);
        let (status, answer) = fetch(&body.to_string())?;
        if status != 200 {
            return Err(read_error(status));
        }
        let json: Value =
            serde_json::from_slice(&answer).map_err(|_| NetError::Proto(ProtoError::Protocol("Google Keep's answer made no sense".into())))?;
        if json.get("error").is_some() {
            let code = json.pointer("/error/code").and_then(Value::as_u64).and_then(|c| u16::try_from(c).ok()).unwrap_or(500);
            return Err(read_error(code));
        }
        if json.get("forceFullResync").and_then(Value::as_bool) == Some(true) {
            // (Keep wants everything again: the host starts over, with no version.)
            download.resync = true;
            return Ok(download);
        }
        if first {
            download.uploaded = sent.clone();
            first = false;
        }
        if let Some(raw) = json.pointer("/userInfo/labels").and_then(Value::as_array) {
            download.labels = Some(raw.iter().filter_map(label_of).take(MAX_KEEP_LABELS).collect());
        }
        for raw in json.get("nodes").and_then(Value::as_array).into_iter().flatten() {
            if raw.get("mergeConflict").is_some() {
                download.conflicts += 1;
                // (The id is still told: the next sync sends it again with Keep's version.)
                if let Some(id) = raw.get("id").and_then(Value::as_str) {
                    download.uploaded.retain(|sent| sent != id);
                }
                continue;
            }
            if download.nodes.len() >= MAX_KEEP_NODES {
                break;
            }
            if let Some(node) = node_of(raw) {
                download.nodes.push(node);
            }
        }
        let to = json.get("toVersion").and_then(Value::as_str).filter(|v| !v.is_empty() && v.len() <= MAX_VERSION);
        let Some(to) = to else {
            return Err(NetError::Proto(ProtoError::Protocol("Google Keep's answer didn't say where it stands".into())));
        };
        download.version = to.to_string();
        if json.get("truncated").and_then(Value::as_bool) != Some(true) {
            download.tidy();
            return Ok(download);
        }
        version = Some(to.to_string());
    }
    // (Too many pages: what was read is kept, and the next run goes on from where this one ended.)
    download.tidy();
    Ok(download)
}

fn request_body(version: Option<&str>, nodes: Vec<Value>, labels: Option<Vec<Value>>, now: Timestamp, session: &str) -> Value {
    let mut body = json!({
        "nodes": nodes,
        "clientTimestamp": stamp(now),
        "requestHeader": {
            "clientSessionId": session,
            "clientPlatform": "ANDROID",
            "clientVersion": {"major": "9", "minor": "9", "build": "9", "revision": "9"},
            "capabilities": [
                {"type": "NC"}, {"type": "PI"}, {"type": "LB"}, {"type": "AN"}, {"type": "SH"}, {"type": "DR"},
                {"type": "TR"}, {"type": "IN"}, {"type": "SNB"}, {"type": "MI"}, {"type": "CO"},
            ],
        },
    });
    if let Some(version) = version {
        body["targetVersion"] = json!(version);
    }
    if let Some(labels) = labels.filter(|l| !l.is_empty()) {
        body["userInfo"] = json!({"labels": labels});
    }
    body
}

/// An instant as Keep writes it (`2026-11-17T12:00:00.000Z`).
fn stamp(at: Timestamp) -> String {
    format!("{}.000Z", DateTime::from_timestamp(at, 0).iso())
}

/// An instant as Keep wrote it, or 0 (also for its epoch: "never").
fn instant(value: Option<&Value>) -> Timestamp {
    let Some(text) = value.and_then(Value::as_str).filter(|t| !t.is_empty() && t.len() <= 64) else { return 0 };
    mailcraft_time::iso_to_timestamp(text, 0).unwrap_or(0).max(0)
}

fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A colour as the mailbox names it: Keep's `DEFAULT` is `white`, the others are in lower case.
fn color_of(keep: &str) -> String {
    if keep.is_empty() || keep == "DEFAULT" { "white".to_string() } else { keep.to_ascii_lowercase() }
}

/// A colour as Keep names it.
fn color_to(color: &str) -> String {
    if color.is_empty() || color == "white" { "DEFAULT".to_string() } else { color.to_ascii_uppercase() }
}

/// A label of Keep's `userInfo`.
fn label_of(raw: &Value) -> Option<KeepLabel> {
    let id = text(raw, "mainId");
    let name = text(raw, "name");
    (!id.is_empty() && !name.is_empty()).then(|| KeepLabel { id: id.to_string(), name: name.to_string() })
}

/// One node of Keep's answer. `None` for what is no node.
pub fn node_of(raw: &Value) -> Option<KeepNode> {
    let id = text(raw, "id");
    if id.is_empty() || !raw.is_object() {
        return None;
    }
    let kind = match text(raw, "type") {
        "NOTE" => KeepKind::Note,
        "LIST" => KeepKind::List,
        "LIST_ITEM" => KeepKind::Item,
        _ => KeepKind::Other,
    };
    let stamps = raw.get("timestamps");
    let at = |name: &str| instant(stamps.and_then(|s| s.get(name)));
    let sort = match raw.get("sortValue") {
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        _ => 0,
    };
    let labels = raw
        .get("labelIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|l| instant(l.get("deleted")) == 0)
        .map(|l| text(l, "labelId").to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut node = KeepNode {
        id: id.to_string(),
        server_id: text(raw, "serverId").to_string(),
        parent_id: text(raw, "parentId").to_string(),
        parent_server_id: text(raw, "parentServerId").to_string(),
        kind,
        // (A node that comes without a parent has gone from Keep.)
        removed: raw.get("parentId").is_none(),
        title: text(raw, "title").to_string(),
        text: text(raw, "text").to_string(),
        checked: raw.get("checked").and_then(Value::as_bool).unwrap_or(false),
        sort,
        color: color_of(text(raw, "color")),
        pinned: raw.get("isPinned").and_then(Value::as_bool).unwrap_or(false),
        archived: raw.get("isArchived").and_then(Value::as_bool).unwrap_or(false),
        trashed: at("trashed") > 0,
        deleted: at("deleted") > 0,
        updated: at("updated"),
        labels,
        raw: String::new(),
    };
    // (What of it is kept as Keep gave it: the node as a whole, only for a node that is kept.)
    if !node.removed && node.kind != KeepKind::Other {
        node.raw = raw.to_string();
    }
    Some(node)
}

/// A label as Keep is told of it.
pub fn label_json(label: &KeepLabel, now: Timestamp) -> Value {
    json!({
        "mainId": label.id,
        "name": label.name,
        "timestamps": {"kind": "notes#timestamps", "created": stamp(now), "updated": stamp(now)},
        "lastMerged": EPOCH,
    })
}

/// A node as Keep is told of it: what it was when Keep gave it (`raw`), with the fields this
/// program keeps put in, or a new node of the Android app's shape.
pub fn node_json(node: &KeepNode, now: Timestamp) -> Value {
    let mut out = serde_json::from_str::<Value>(&node.raw).ok().filter(Value::is_object).unwrap_or_else(|| fresh(node, now));
    let kind = match node.kind {
        KeepKind::Note => "NOTE",
        KeepKind::List => "LIST",
        KeepKind::Item => "LIST_ITEM",
        KeepKind::Other => "BLOB",
    };
    out["id"] = json!(node.id);
    out["kind"] = json!("notes#node");
    out["type"] = json!(kind);
    out["parentId"] = json!(if node.parent_id.is_empty() { "root" } else { node.parent_id.as_str() });
    out["sortValue"] = json!(node.sort);
    out["text"] = json!(node.text);
    if !node.server_id.is_empty() {
        out["serverId"] = json!(node.server_id);
    }
    let mut stamps = out.get("timestamps").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    stamps["kind"] = json!("notes#timestamps");
    if stamps.get("created").is_none() {
        stamps["created"] = json!(stamp(now));
    }
    stamps["updated"] = json!(stamp(now));
    stamps["userEdited"] = json!(stamp(now));
    // Trashed and deleted are times: the epoch is "no".
    stamps["trashed"] = json!(if node.trashed { stamp(now) } else { EPOCH.to_string() });
    if node.deleted {
        stamps["deleted"] = json!(stamp(now));
    } else if let Some(map) = stamps.as_object_mut() {
        map.remove("deleted");
    }
    out["timestamps"] = stamps;
    match node.kind {
        KeepKind::Item => {
            out["checked"] = json!(node.checked);
            if !node.parent_server_id.is_empty() {
                out["parentServerId"] = json!(node.parent_server_id);
            } else if out.get("parentServerId").is_none() {
                out["parentServerId"] = Value::Null;
            }
            if out.get("superListItemId").is_none() {
                out["superListItemId"] = Value::Null;
            }
        }
        _ => {
            out["title"] = json!(node.title);
            out["color"] = json!(color_to(&node.color));
            out["isArchived"] = json!(node.archived);
            out["isPinned"] = json!(node.pinned);
            // The labels: those it has now, and those it had that it no longer has, as removed.
            let mut ids: Vec<Value> = node.labels.iter().map(|l| json!({"labelId": l, "deleted": EPOCH})).collect();
            let earlier: Vec<String> =
                out.get("labelIds").and_then(Value::as_array).into_iter().flatten().map(|l| text(l, "labelId").to_string()).collect();
            for gone in earlier.iter().filter(|l| !l.is_empty() && !node.labels.contains(l)) {
                ids.push(json!({"labelId": gone, "deleted": stamp(now)}));
            }
            if ids.is_empty() {
                if let Some(map) = out.as_object_mut() {
                    map.remove("labelIds");
                }
            } else {
                out["labelIds"] = json!(ids);
            }
        }
    }
    out
}

/// A node that Keep has never seen, in the shape the Android app sends.
fn fresh(node: &KeepNode, now: Timestamp) -> Value {
    json!({
        "kind": "notes#node",
        "timestamps": {"kind": "notes#timestamps", "created": stamp(now), "updated": stamp(now), "userEdited": stamp(now)},
        "nodeSettings": {"newListItemPlacement": "BOTTOM", "graveyardState": "COLLAPSED", "checkedListItemsPolicy": "GRAVEYARD"},
        "annotationsGroup": {"kind": "notes#annotationsGroup"},
        "collaborators": [],
        "id": node.id,
    })
}
