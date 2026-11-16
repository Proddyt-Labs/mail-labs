// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Internet calendars: a calendar whose events are those of an `.ics` file at a web address.
//!
//! The engine asks nothing of any server. `calendar.subscribe` makes a calendar that can only be
//! read, and marks it stale; the window (or any host with sockets: `mailcraft_net::calendar_feed`)
//! reads the addresses of the calendars that are due ([`Subscription::due`]: stale, never tried,
//! or not tried for three hours) and hands each answer to `calendar.applyFeed`, which replaces
//! that calendar's events with the feed's. `calendar.refreshSubscriptions` marks every feed stale,
//! which is how Send/Receive and an agent ask for a read now.
//!
//! The replacement goes by UID, so an event that is the same in the feed keeps its identity (its
//! id, and whatever points at it); one that changed is changed in place; one the feed no longer
//! has goes. A feed that couldn't be read leaves the events as they are and says why in
//! [`Subscription::last_error`].

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use mailcraft_model::{Calendar, Event, Id, MAX_FEED_ERROR, MAX_FEED_EVENTS, MAX_FEEDS, Mailbox, Rights, Subscription, check_feed_url, clean_name};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calendar.subscribe", "Add Calendar from Internet", "Home › Manage Calendars", subscribe)
            .params(r##"{"url": "https://… or http:// to a private network, to an .ics file", "name": string, "color"?: "#RRGGBB", "group"?: string} — a calendar that can only be read, whose events are the file's, read again every three hours; listed under `group` when there is one; answers {"id"}"##),
        CommandSpec::new("calendar.setGroup", "Move to Group", "Folder › Actions", set_group).params(
            r#"{"id": number, "group": string ("" for none)} — the heading a calendar kept on this computer (an internet calendar too) is listed under; a server's calendars keep their server's groups"#,
        ),
        CommandSpec::new("calendar.rename", "Rename Calendar", "Folder › Actions", rename)
            .params(r#"{"id": number, "name": string} — a calendar kept on this computer (an internet calendar too); a server's calendars keep their server's names"#),
        CommandSpec::new("calendar.refreshSubscriptions", "Update Internet Calendars", "Send / Receive", refresh)
            .pure()
            .params(r#"{} — marks every internet calendar to be read at once; the window does it (answers {"subscriptions": number})"#),
        CommandSpec::new("calendar.unsubscribe", "Remove Internet Calendar", "Folder › Actions", unsubscribe).params(r#"{"id": number}"#),
        CommandSpec::new("calendar.applyFeed", "Apply Internet Calendar", "Send / Receive", apply_feed)
            .pure()
            .params(r#"{"id": number, "ics": string (what the address answered) | "error": string (why it couldn't be read)} — what a host does with an internet calendar's address; the events are replaced by UID"#),
    ]
}

/// What a replacement did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
}

fn subscribe(s: &mut Session, v: &Value) -> CmdResult {
    let url = p::req_str(v, "url")?.trim().to_string();
    check_feed_url(&url).map_err(|e| bad(e.to_string()))?;
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    if s.mailbox.calendars.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
        return Err(failed!("a calendar named \"{name}\" already exists", name));
    }
    let feeds = s.mailbox.calendars.iter().filter_map(|c| c.subscription.as_ref());
    if feeds.clone().any(|f| f.url == url) {
        return Err(fail("that address is subscribed to already"));
    }
    if feeds.count() >= MAX_FEEDS || s.mailbox.calendars.len() >= 200 {
        return Err(fail("there are too many calendars"));
    }
    let color = match p::str(v, "color") {
        Some(c) => mailcraft_model::Rgb::parse(c).ok_or_else(|| bad("`color` looks like #2E8B57"))?,
        None => super::calendar::next_color(&s.mailbox),
    };
    let group = match p::str(v, "group") {
        Some(g) => clean_name(g).unwrap_or_default(),
        None => String::new(),
    };
    let id = s.mailbox.alloc_id();
    let subscription = Some(Subscription { url, stale: true, ..Default::default() });
    s.mailbox.calendars.push(Calendar { id, name, color, group, visible: true, rights: Rights::Read, subscription, ..Default::default() });
    Ok(json!({"id": id}))
}

/// The calendar `id` when it is one this computer keeps (no server lists it): its name and group
/// are the owner's to change here.
fn kept_here(s: &mut Session, id: Id) -> Result<&mut Calendar, CmdError> {
    let mailbox = &s.mailbox;
    let Some(calendar) = mailbox.calendar(id) else { return Err(failed!("no calendar with id {id}", id)) };
    if super::remote::keeper(mailbox, &calendar.origin).is_some() {
        return Err(fail("that calendar is a server's: its name and group are the server's"));
    }
    Ok(s.mailbox.calendars.iter_mut().find(|c| c.id == id).expect("found above"))
}

fn set_group(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let group = clean_name(p::req_str(v, "group")?).unwrap_or_default();
    // A handful of headings at most, as for a server's groups.
    let mailbox = &s.mailbox;
    let mut headings: HashSet<String> = mailbox.calendars.iter().filter(|c| c.id != id).map(|c| c.group.to_lowercase()).collect();
    headings.insert(group.to_lowercase());
    if headings.len() > super::calendar_settings::MAX_GROUPS + 1 {
        return Err(fail("there are too many groups"));
    }
    let calendar = kept_here(s, id)?;
    calendar.group = group.clone();
    Ok(json!({"id": id, "group": group}))
}

fn rename(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    if s.mailbox.calendars.iter().any(|c| c.id != id && c.name.eq_ignore_ascii_case(&name)) {
        return Err(failed!("a calendar named \"{name}\" already exists", name));
    }
    let calendar = kept_here(s, id)?;
    calendar.name = name.clone();
    Ok(json!({"id": id, "name": name}))
}

fn refresh(s: &mut Session, _: &Value) -> CmdResult {
    let mut n = 0;
    for feed in s.mailbox.calendars.iter_mut().filter_map(|c| c.subscription.as_mut()) {
        feed.stale = true;
        n += 1;
    }
    Ok(json!({"subscriptions": n}))
}

fn unsubscribe(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    if !s.mailbox.calendar(id).is_some_and(|c| c.subscription.is_some()) {
        return Err(failed!("calendar {id} isn't an internet calendar", id));
    }
    s.mailbox.calendars.retain(|c| c.id != id);
    let events: Vec<Id> = s.mailbox.events.values().filter(|e| e.calendar == id).map(|e| e.id).collect();
    for e in &events {
        s.mailbox.remove_event(*e)?;
    }
    if s.view.in_calendar == id {
        s.view.in_calendar = 0;
    }
    Ok(json!({"deleted": id, "events": events.len()}))
}

fn apply_feed(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let now = s.now();
    if !s.mailbox.calendar(id).is_some_and(|c| c.subscription.is_some()) {
        return Err(failed!("calendar {id} isn't an internet calendar", id));
    }
    // What is wrong with the answer is kept with the calendar, and the events stay: an error
    // here would undo the note, and a dialog would come up on every try.
    let outcome = match (p::str(v, "ics"), p::str(v, "error")) {
        (Some(text), _) => read(s, text),
        (None, Some(why)) => Err(tidy_error(why)),
        (None, None) => return Err(bad("`ics` or `error` is required")),
    };
    let merged = match outcome {
        Ok(events) => Ok(merge(&mut s.mailbox, id, events)?),
        Err(why) => Err(why),
    };
    let Some(feed) = s.mailbox.calendars.iter_mut().find(|c| c.id == id).and_then(|c| c.subscription.as_mut()) else {
        return Err(fail("the calendar is gone"));
    };
    feed.stale = false;
    feed.last_attempt = now;
    match merged {
        Ok(m) => {
            feed.last_refresh = now;
            feed.last_error.clear();
            Ok(json!({"id": id, "ok": true, "added": m.added, "updated": m.updated, "unchanged": m.unchanged, "removed": m.removed}))
        }
        Err(why) => {
            feed.last_error = why.clone();
            Ok(json!({"id": id, "ok": false, "error": why}))
        }
    }
}

/// An error as it is kept: one line, no control characters, not long.
fn tidy_error(text: &str) -> String {
    let line: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = if line.is_empty() { "the calendar couldn't be read".to_string() } else { line };
    line.chars().take(MAX_FEED_ERROR).collect()
}

/// The events in an iCalendar text, as they are kept here; the text isn't one when it has no
/// `BEGIN:VCALENDAR`.
fn read(s: &Session, text: &str) -> Result<Vec<Event>, String> {
    if !text.to_ascii_uppercase().contains("BEGIN:VCALENDAR") {
        return Err("what the server sent isn't an iCalendar file".into());
    }
    let offset_at = super::remote::offset_of(s);
    let file = mailcraft_ical::parse(text, &offset_at);
    Ok(file.events.into_iter().take(MAX_FEED_EVENTS).map(|e| super::calendar::tidy_imported(e, &offset_at)).collect())
}

/// What identifies an event of a feed: its UID (one that is an instance of a series is already
/// told apart by [`super::calendar::tidy_imported`]), or what it says when it has none.
fn key_of(e: &Event) -> String {
    if e.uid.is_empty() { format!("~{}|{:?}|{:?}", e.title, e.start, e.end) } else { e.uid.clone() }
}

/// Replace the events of `calendar` by `events`: the same UID keeps its id (and is changed in
/// place only when something differs), a new one is added, one that isn't there any more is
/// removed.
pub fn merge(mailbox: &mut Mailbox, calendar: Id, events: Vec<Event>) -> Result<Merged, CmdError> {
    let mut have: HashMap<String, Id> = HashMap::new();
    let mut stray: Vec<Id> = Vec::new();
    let mut ids: Vec<(Id, String)> = mailbox.events.values().filter(|e| e.calendar == calendar).map(|e| (e.id, key_of(e))).collect();
    ids.sort_unstable();
    for (id, key) in ids {
        match have.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert(id);
            }
            Entry::Occupied(_) => stray.push(id),
        }
    }
    let mut merged = Merged::default();
    let mut seen: HashSet<String> = HashSet::new();
    for mut e in events {
        let key = key_of(&e);
        if !seen.insert(key.clone()) {
            continue;
        }
        e.calendar = calendar;
        match have.remove(&key) {
            Some(id) => {
                e.id = id;
                if mailbox.event(id) == Some(&e) {
                    merged.unchanged += 1;
                } else {
                    *mailbox.event_mut(id)? = e;
                    merged.updated += 1;
                }
            }
            None => {
                e.id = 0;
                mailbox.add_event(e)?;
                merged.added += 1;
            }
        }
    }
    for id in have.into_values().chain(stray) {
        mailbox.remove_event(id)?;
        merged.removed += 1;
    }
    Ok(merged)
}
