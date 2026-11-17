// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: Google Keep, the owner's notes at Google, kept in step both ways. Plain data
//! like [`crate::remote_tasks`]: a host with sockets brings a [`KeepDownload`] and takes a
//! [`KeepUpload`] to Google, and the engine merges the one and works out the other
//! (`sync.applyKeep`, `sync.keepChanges`).
//!
//! **No secret is ever in here.** A Keep sign-in (the master token, the access token) lives in the
//! system's credential store and in the memory of the one job that uses it; what is kept in the
//! mailbox is notes, labels and the version of Keep they were read at (a position in Keep's list
//! of changes, which opens nothing).
//!
//! All of it is hostile: it comes from a server, or from a command's parameters.
//! [`KeepNode::tidy`] cuts a node to size before any of it is kept.

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::remote::{key, line, text};
use crate::{Account, Id};

/// Most nodes (notes, checklist items) one download is taken to hold, and most one upload carries.
pub const MAX_KEEP_NODES: usize = 50_000;
pub const MAX_KEEP_UP: usize = 200;
/// Most labels, and most items in one checklist.
pub const MAX_KEEP_LABELS: usize = 500;
pub const MAX_NOTE_ITEMS: usize = 2_000;
const MAX_TITLE: usize = 1_000;
const MAX_BODY: usize = 100_000;
const MAX_RAW: usize = 64 * 1024;
/// Longest label name.
pub const MAX_LABEL: usize = 50;

/// A sign-in secret: the cookie of Google's sign-in page, a master token, an access token. It
/// shows itself nowhere (not in `Debug`, not in `Display`) and is overwritten in memory when it
/// goes. Holding one in a type makes that type's `Debug` safe too.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(value)
    }

    /// The secret itself: for the one place that sends it, and the credential store.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[hidden]")
    }
}

/// A checklist item, or the body of a text note, as Keep keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NoteItem {
    /// What Keep's protocol calls the node (the id a client makes up for a new one).
    pub id: String,
    pub text: String,
    pub checked: bool,
    /// Keep sorts a list from the largest down.
    pub sort: i64,
    /// The node as Keep sent it, so that what this program doesn't know of it goes back as it came.
    pub raw: String,
}

/// What a note has that Keep keeps, as one comparable thing: the note as it is now against as it
/// was when Keep last had it tells what was changed here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NoteFields {
    pub title: String,
    pub text: String,
    pub checklist: bool,
    /// The checklist, in order: (text, checked).
    pub items: Vec<(String, bool)>,
    pub color: String,
    pub pinned: bool,
    pub archived: bool,
    pub trashed: bool,
    pub categories: Vec<String>,
}

/// What ties a note to Keep.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeepLink {
    /// The account whose Keep keeps it.
    pub account: Id,
    /// The node's id in Keep's protocol.
    pub id: String,
    /// The note node as Keep sent it.
    pub raw: String,
    /// A text note's body is a node of its own in Keep.
    pub body: Option<NoteItem>,
    /// The fields as Keep last had them; `None` for a note that was made here and isn't there yet.
    pub synced: Option<NoteFields>,
    /// When Keep says the note was last changed (the node's `updated`).
    pub server_updated: Timestamp,
    /// Deleted for good here: Keep is yet to be told.
    pub deleted: bool,
    /// Checklist items taken out here that Keep is yet to be told about.
    pub removed: Vec<NoteItem>,
}

/// A label as Keep has it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeepLabel {
    pub id: String,
    pub name: String,
}

/// What is kept of an account's Keep besides its notes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeepState {
    /// Where in Keep's list of changes the notes were read to (not a secret).
    pub version: String,
    pub labels: Vec<KeepLabel>,
}

/// What kind of node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepKind {
    #[default]
    Note,
    List,
    Item,
    /// A picture, a drawing or a sound: left untouched.
    Other,
}

/// One node of Keep's tree, as it is read or sent: a note, a checklist or an item of either.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeepNode {
    pub id: String,
    pub server_id: String,
    pub parent_id: String,
    /// What Keep calls the parent, when it is known (an item under a note that Keep has).
    pub parent_server_id: String,
    pub kind: KeepKind,
    /// The node went from Keep (it came without a parent).
    pub removed: bool,
    pub title: String,
    pub text: String,
    pub checked: bool,
    pub sort: i64,
    /// A colour name in lower case (`white` for Keep's default).
    pub color: String,
    pub pinned: bool,
    pub archived: bool,
    pub trashed: bool,
    /// Deleted for good (a note that reached the end of the Trash).
    pub deleted: bool,
    pub updated: Timestamp,
    /// The label ids of a note.
    pub labels: Vec<String>,
    /// The node as Keep sent it ([`NoteItem::raw`]).
    pub raw: String,
}

impl KeepNode {
    /// Cut everything to the size the mailbox keeps.
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
        self.server_id = key(&self.server_id);
        self.parent_id = key(&self.parent_id);
        self.parent_server_id = key(&self.parent_server_id);
        self.title = line(&self.title, MAX_TITLE);
        self.text = text(&self.text, MAX_BODY);
        self.color = line(&self.color, 20).to_lowercase();
        self.labels = self.labels.iter().map(|l| key(l)).filter(|l| !l.is_empty()).take(MAX_KEEP_LABELS).collect();
        if self.raw.len() > MAX_RAW {
            self.raw.clear();
        }
    }
}

/// What a host brought from Keep for one account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeepDownload {
    pub account: Id,
    pub email: String,
    /// Where Keep's list of changes is now.
    pub version: String,
    /// Everything was read (there was no version to start from): what is here and not in it is gone.
    pub full: bool,
    /// Keep wants everything read again from the start.
    pub resync: bool,
    pub nodes: Vec<KeepNode>,
    /// The account's labels, when Keep said them.
    pub labels: Option<Vec<KeepLabel>>,
    /// The ids of the nodes this run told Keep about, which Keep took.
    pub uploaded: Vec<String>,
    /// Nodes Keep wouldn't merge (changed there at the same time).
    pub conflicts: usize,
}

impl KeepDownload {
    /// The download for `account` as the job had it, never with another's address.
    pub fn for_account(mut self, account: &Account) -> KeepDownload {
        self.account = account.id;
        self.email = account.email.clone();
        self
    }

    /// Cut every node to size, and keep no more than the most.
    pub fn tidy(&mut self) {
        self.nodes.truncate(MAX_KEEP_NODES);
        for node in &mut self.nodes {
            node.tidy();
        }
        self.version = key(&self.version);
        self.uploaded = self.uploaded.iter().map(|u| key(u)).filter(|u| !u.is_empty()).take(MAX_KEEP_NODES).collect();
        if let Some(labels) = &mut self.labels {
            labels.truncate(MAX_KEEP_LABELS);
            for label in labels.iter_mut() {
                label.id = key(&label.id);
                label.name = line(&label.name, MAX_LABEL);
            }
            labels.retain(|l| !l.id.is_empty() && !l.name.is_empty());
        }
    }
}

/// What a host is to tell Keep for one account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeepUpload {
    pub account: Id,
    pub email: String,
    /// The version the notes here were read at (empty: Keep is read from the start).
    pub version: String,
    pub nodes: Vec<KeepNode>,
    /// The labels, when one is new or changed.
    pub labels: Option<Vec<KeepLabel>>,
}
