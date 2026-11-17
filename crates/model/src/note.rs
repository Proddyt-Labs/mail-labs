// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Notes and journal entries, as they come from a .pst file (#815): kept, listed and searched.
//! Proddyt Labs fork: a note may also be a Google Keep note or checklist ([`crate::keep`]).

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Address, Id};

/// A sticky note.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Note {
    pub id: Id,
    pub text: String,
    /// `blue`, `green`, `pink`, `yellow` or `white`.
    pub color: String,
    pub categories: Vec<String>,
    pub created: Timestamp,
    pub modified: Timestamp,
    /// What it is known by in the file it came from, so that a second import skips it.
    pub imported: String,
    /// Proddyt Labs fork: the title (Keep notes have one; a note from a .pst file has none).
    pub title: String,
    /// A checklist: `items` is what it holds and `text` is not used.
    pub checklist: bool,
    pub items: Vec<crate::NoteItem>,
    pub pinned: bool,
    pub archived: bool,
    pub trashed: bool,
    /// What ties it to Google Keep; `None` for a note that exists only on this computer.
    pub keep: Option<crate::KeepLink>,
}

impl Note {
    /// The fields Keep keeps, as the note has them now.
    pub fn keep_fields(&self) -> crate::NoteFields {
        crate::NoteFields {
            title: self.title.clone(),
            text: if self.checklist { String::new() } else { self.text.clone() },
            checklist: self.checklist,
            items: if self.checklist { self.items.iter().map(|i| (i.text.clone(), i.checked)).collect() } else { Vec::new() },
            color: self.color.clone(),
            pinned: self.pinned,
            archived: self.archived,
            trashed: self.trashed,
            categories: self.categories.clone(),
        }
    }

    /// Whether it was changed here since Keep last had it (or was made here).
    pub fn keep_dirty(&self) -> bool {
        self.keep.as_ref().is_some_and(|k| k.deleted || !k.removed.is_empty() || k.synced.as_ref() != Some(&self.keep_fields()))
    }
}

/// A record of something done, and when.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JournalEntry {
    pub id: Id,
    pub subject: String,
    /// What kind of thing it records, in the file's words (`Phone call`).
    pub activity: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub minutes: u32,
    pub text: String,
    pub people: Vec<Address>,
    pub categories: Vec<String>,
    pub imported: String,
}
