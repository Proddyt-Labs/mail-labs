// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Notes module. The owner's notes (Google Keep's, and those from a .pst
//! file) are listed, pinned first, beside an editor for the one selected. Every change is a
//! `notes.*` command, so the window, an agent and the Keep sync see the same thing. The editor
//! keeps what is typed until it is saved, and a note is saved before another one is opened.
//!
//! The bar at the top says whether the account's notes are kept in step with Google Keep and
//! offers to connect or disconnect it. Nothing here holds a sign-in: that is [`crate::keep_sync`]'s.

use egui::{RichText, Stroke, Ui};
use mailcraft_engine::cmd::keep as keep_cmd;
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::Tokens;

/// The Notes module's state: the note in the editor, and the list's filters.
#[derive(Default)]
pub(crate) struct NotesUi {
    /// The note in the editor.
    selected: Option<Id>,
    /// What the editor holds for `selected`, when it is loaded.
    draft: Option<Draft>,
    /// `active` (the default), `archived` or `trashed`.
    state: Option<String>,
    /// Only the notes with this label.
    label: Option<String>,
    /// What the search box holds.
    query: String,
    /// The owner asked to delete the trashed note for good; the next click does it.
    confirm_forever: bool,
}

/// What the editor holds: the fields as they are typed, until saved.
#[derive(Clone, Default)]
struct Draft {
    id: Id,
    title: String,
    text: String,
    checklist: bool,
    items: Vec<DraftItem>,
    labels: String,
    pinned: bool,
    archived: bool,
    trashed: bool,
    dirty: bool,
}

#[derive(Clone, Default)]
struct DraftItem {
    /// Keep's id for the item; empty for one not yet saved.
    id: String,
    text: String,
    checked: bool,
}

/// A listed note, as the list shows it.
struct Row {
    id: Id,
    title: String,
    snippet: String,
    pinned: bool,
    waiting: bool,
}

/// What the editor's buttons ask for once it has been drawn.
#[derive(Clone, Copy)]
enum Act {
    Save,
    Revert,
    Forever,
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn flag_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

impl Draft {
    fn load(app: &mut MailApp, id: Id) -> Option<Draft> {
        let v = app.run("notes.get", json!({"id": id})).ok()?;
        let items = v
            .get("itemList")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(|i| DraftItem { id: text_of(i, "id"), text: text_of(i, "text"), checked: flag_of(i, "checked") }).collect())
            .unwrap_or_default();
        let labels = v
            .get("categories")
            .and_then(Value::as_array)
            .map(|c| c.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        Some(Draft {
            id,
            title: text_of(&v, "title"),
            text: text_of(&v, "text"),
            checklist: flag_of(&v, "checklist"),
            items,
            labels,
            pinned: flag_of(&v, "pinned"),
            archived: flag_of(&v, "archived"),
            trashed: flag_of(&v, "trashed"),
            dirty: false,
        })
    }

    /// What `notes.update` is told: every field as it is typed now.
    fn update(&self) -> Value {
        let labels: Vec<&str> = self.labels.split(',').map(str::trim).filter(|l| !l.is_empty()).collect();
        let mut p = json!({
            "id": self.id,
            "title": self.title,
            "pinned": self.pinned,
            "archived": self.archived,
            "trashed": self.trashed,
            "categories": labels,
        });
        if self.checklist {
            p["items"] = json!(
                self.items
                    .iter()
                    .map(|i| {
                        let mut item = json!({"text": i.text, "checked": i.checked});
                        if !i.id.is_empty() {
                            item["id"] = json!(i.id);
                        }
                        item
                    })
                    .collect::<Vec<_>>()
            );
        } else {
            p["text"] = json!(self.text);
        }
        p
    }
}

/// The Notes module, in the module's place: the Google Keep bar, then the list and the editor.
pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("notes_keep").frame(egui::Frame::NONE.fill(t.nav)).show(ui, |ui| keep_bar(app, ui));
    let (rows, labels) = listing(app);
    let frame = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border));
    egui::Panel::left("notes_list")
        .default_size(300.0)
        .size_range(200.0..=520.0)
        .resizable(true)
        .frame(frame)
        .show(ui, |ui| list(app, ui, &rows, &labels));
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| editor(app, ui));
}

/// Whether Google Keep keeps the owner's notes, and the button that connects or disconnects it.
fn keep_bar(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        let accounts = keep_cmd::keep_accounts(&app.session.mailbox);
        if let Some(&account) = accounts.first() {
            let line = app.gkeep_line(account).map(|(line, _)| line).unwrap_or_default();
            ui.label(RichText::new(line).color(t.text_dim));
            if ui.button(tl!("Disconnect Google Keep").to_string()).clicked() {
                app.disconnect_google_keep(account);
            }
        } else {
            ui.label(RichText::new(tl!("Notes stay on this computer.").to_string()).color(t.text_dim));
            if let Some(account) = app.gkeep_offer_account()
                && ui.button(tl!("Connect Google Keep").to_string()).clicked()
            {
                app.ask_google_keep(account);
            }
        }
    });
    ui.add_space(4.0);
}

/// The notes in view under the filters, pinned first, and the labels the notes have.
fn listing(app: &mut MailApp) -> (Vec<Row>, Vec<String>) {
    let state = app.notes_ui.state.clone().unwrap_or_else(|| "active".to_string());
    let mut args = json!({"state": state, "query": app.notes_ui.query, "limit": 1000});
    if let Some(label) = &app.notes_ui.label {
        args["label"] = json!(label);
    }
    let Ok(v) = app.run("notes.list", args) else { return (Vec::new(), Vec::new()) };
    let rows = v
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n.get("id").and_then(Value::as_u64).filter(|id| *id != 0)?;
            Some(Row {
                id,
                title: text_of(n, "subject"),
                snippet: text_of(n, "snippet").chars().take(90).collect(),
                pinned: flag_of(n, "pinned"),
                waiting: flag_of(n, "waiting"),
            })
        })
        .collect();
    let labels = v.get("labels").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    (rows, labels)
}

/// Move the editor to another note (or none), saving what is typed first. A note that won't save
/// stays open, with what is typed.
fn select(app: &mut MailApp, id: Option<Id>) {
    if app.notes_ui.selected == id {
        return;
    }
    if !save(app) {
        return;
    }
    app.notes_ui.selected = id;
    app.notes_ui.draft = None;
    app.notes_ui.confirm_forever = false;
}

/// Save the editor's note when something in it was changed. `true` when nothing is left unsaved.
fn save(app: &mut MailApp) -> bool {
    let Some(draft) = app.notes_ui.draft.as_ref() else { return true };
    if !draft.dirty {
        return true;
    }
    let payload = draft.update();
    match app.run("notes.update", payload) {
        Ok(_) => {
            // Reloaded from the mailbox next frame: what is shown is then what was saved.
            app.notes_ui.draft = None;
            true
        }
        Err(e) => {
            app.status(e);
            false
        }
    }
}

fn new_note(app: &mut MailApp) {
    if !save(app) {
        return;
    }
    match app.run("notes.create", json!({"title": tl!("New note").to_string()})) {
        Ok(v) => {
            app.notes_ui.state = None;
            app.notes_ui.selected = v.get("id").and_then(Value::as_u64);
            app.notes_ui.draft = None;
        }
        Err(e) => app.status(e),
    }
}

/// The list: the search box, the states, the labels, and the notes.
fn list(app: &mut MailApp, ui: &mut Ui, rows: &[Row], labels: &[String]) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::singleline(&mut app.notes_ui.query).hint_text(tl!("Search notes").to_string()).desired_width(ui.available_width() - 16.0),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.add_space(8.0);
        let current = app.notes_ui.state.clone().unwrap_or_else(|| "active".to_string());
        for (key, label) in [("active", tl!("Notes")), ("archived", tl!("Archived")), ("trashed", tl!("Trash"))] {
            if ui.selectable_label(current == key, label.to_string()).clicked() {
                app.notes_ui.state = Some(key.to_string());
                app.notes_ui.confirm_forever = false;
            }
        }
    });
    if !labels.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(8.0);
            for label in labels {
                let on = app.notes_ui.label.as_deref() == Some(label.as_str());
                if ui.selectable_label(on, label.as_str()).clicked() {
                    app.notes_ui.label = if on { None } else { Some(label.clone()) };
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.button(tl!("New note").to_string()).clicked() {
            new_note(app);
        }
    });
    ui.add_space(6.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if rows.is_empty() {
            ui.add_space(12.0);
            ui.label(RichText::new(tl!("No notes here.").to_string()).color(t.text_dim));
        }
        for row in rows {
            let selected = app.notes_ui.selected == Some(row.id);
            let head = if row.title.trim().is_empty() { tl!("(no title)").to_string() } else { row.title.clone() };
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if ui.selectable_label(selected, RichText::new(head).strong()).clicked() {
                    select(app, Some(row.id));
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(RichText::new(&row.snippet).color(t.text_dim).size(12.0));
            });
            if row.pinned || row.waiting {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    if row.pinned {
                        ui.label(RichText::new(tl!("Pinned").to_string()).color(t.accent_text).size(11.0));
                    }
                    if row.waiting {
                        ui.label(RichText::new(tl!("Waiting to go to Google Keep").to_string()).color(t.text_dim).size(11.0));
                    }
                });
            }
            ui.add_space(4.0);
        }
    });
}

/// The editor: the buttons, then the title, the body or the checklist, and the labels.
fn editor(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = app.notes_ui.selected else {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| ui.label(RichText::new(tl!("Choose a note, or make a new one.").to_string()).color(t.text_dim)));
        return;
    };
    if app.notes_ui.draft.as_ref().is_none_or(|d| d.id != id) {
        match Draft::load(app, id) {
            Some(loaded) => app.notes_ui.draft = Some(loaded),
            None => {
                // Gone from the mailbox (or never there): nothing to show.
                app.notes_ui.selected = None;
                return;
            }
        }
    }
    let Some(mut d) = app.notes_ui.draft.take() else { return };
    let mut act: Option<Act> = None;
    let confirm = app.notes_ui.confirm_forever;
    let mut ask = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut changed = false;
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(8.0);
            if ui.add_enabled(d.dirty, egui::Button::new(tl!("Save").to_string())).clicked() {
                act = Some(Act::Save);
            }
            if ui.add_enabled(d.dirty, egui::Button::new(tl!("Revert").to_string())).clicked() {
                act = Some(Act::Revert);
            }
            let pin = if d.pinned { tl!("Unpin") } else { tl!("Pin") };
            if ui.button(pin.to_string()).clicked() {
                d.pinned = !d.pinned;
                d.dirty = true;
                act = Some(Act::Save);
            }
            let archive = if d.archived { tl!("Unarchive") } else { tl!("Archive") };
            if !d.trashed && ui.button(archive.to_string()).clicked() {
                d.archived = !d.archived;
                d.dirty = true;
                act = Some(Act::Save);
            }
            let trash = if d.trashed { tl!("Restore") } else { tl!("Move to Trash") };
            if ui.button(trash.to_string()).clicked() {
                d.trashed = !d.trashed;
                d.dirty = true;
                act = Some(Act::Save);
            }
        });
        if d.trashed {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(8.0);
                if confirm {
                    ui.label(RichText::new(tl!("Delete this note for good, in Google Keep too?").to_string()).color(t.text_dim));
                    if ui.button(tl!("Yes, delete for good").to_string()).clicked() {
                        act = Some(Act::Forever);
                    }
                } else if ui.button(tl!("Delete for good").to_string()).clicked() {
                    // (The first click asks; only the second one deletes.)
                    ask = true;
                }
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            changed |= ui
                .add(egui::TextEdit::singleline(&mut d.title).hint_text(tl!("Title").to_string()).desired_width(ui.available_width() - 16.0))
                .changed();
        });
        ui.add_space(8.0);
        if d.checklist {
            let mut remove = None;
            for (at, item) in d.items.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    changed |= ui.checkbox(&mut item.checked, "").changed();
                    changed |= ui.add(egui::TextEdit::singleline(&mut item.text).desired_width(ui.available_width() - 90.0)).changed();
                    if ui.small_button(tl!("Remove").to_string()).clicked() {
                        remove = Some(at);
                    }
                });
            }
            if let Some(at) = remove {
                d.items.remove(at);
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if ui.button(tl!("Add item").to_string()).clicked() {
                    d.items.push(DraftItem::default());
                    changed = true;
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                changed |= ui
                    .add(
                        egui::TextEdit::multiline(&mut d.text)
                            .desired_rows(16)
                            .desired_width(ui.available_width() - 16.0)
                            .hint_text(tl!("Note").to_string()),
                    )
                    .changed();
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut d.labels)
                        .hint_text(tl!("Labels, separated by commas").to_string())
                        .desired_width(ui.available_width() - 16.0),
                )
                .changed();
        });
        if changed {
            d.dirty = true;
        }
    });
    if ask {
        app.notes_ui.confirm_forever = true;
    }
    app.notes_ui.draft = Some(d);
    match act {
        Some(Act::Save) => {
            save(app);
        }
        Some(Act::Revert) => app.notes_ui.draft = None,
        Some(Act::Forever) => delete_forever(app),
        None => {}
    }
}

fn delete_forever(app: &mut MailApp) {
    let Some(id) = app.notes_ui.selected else { return };
    match app.run("notes.deleteForever", json!({"id": id})) {
        Ok(_) => {
            app.notes_ui.selected = None;
            app.notes_ui.draft = None;
            app.notes_ui.confirm_forever = false;
        }
        Err(e) => app.status(e),
    }
}
