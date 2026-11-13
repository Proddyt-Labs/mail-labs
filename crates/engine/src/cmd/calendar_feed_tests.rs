// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Internet calendars: subscribing, replacing a calendar's events by those of a feed (by UID),
//! a feed that can't be read, and that what comes from an address can't be changed here.

use mailcraft_model::{Calendar, Event, Id, Rights};
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{Access, Class, access};

const NOW: i64 = 1_791_993_600;
const URL: &str = "http://100.87.104.117:8241/secret/google/pessoal.ics";

/// A feed as a server writes it: CRLF, an all-day event (`VALUE=DATE`) and a timed one in UTC.
fn feed(events: &[(&str, &str, &str)]) -> String {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Feeds//EN\r\nX-WR-CALNAME:Pessoal\r\n");
    for (uid, title, when) in events {
        text.push_str(&format!("BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261101T000000Z\r\n{when}\r\nSUMMARY:{title}\r\nLOCATION:Sala 1\r\nDESCRIPTION:Linha um\\nLinha dois\r\nEND:VEVENT\r\n"));
    }
    text.push_str("END:VCALENDAR\r\n");
    text
}

const ALL_DAY: &str = "DTSTART;VALUE=DATE:20261120\r\nDTEND;VALUE=DATE:20261121";
const TIMED: &str = "DTSTART:20261121T200000Z\r\nDTEND:20261121T213000Z";
const LATER: &str = "DTSTART:20261122T120000Z\r\nDTEND:20261122T130000Z";

fn session() -> Session {
    Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
}

fn subscribe(s: &mut Session) -> Id {
    s.run("calendar.subscribe", &json!({"url": URL, "name": "Pessoal"})).unwrap()["id"].as_u64().unwrap()
}

fn apply(s: &mut Session, id: Id, ics: &str) -> Value {
    s.run("calendar.applyFeed", &json!({"id": id, "ics": ics})).unwrap()
}

fn events_of(s: &Session, calendar: Id) -> Vec<Event> {
    let mut found: Vec<Event> = s.mailbox.events.values().filter(|e| e.calendar == calendar).map(|e| (**e).clone()).collect();
    found.sort_by(|a, b| a.uid.cmp(&b.uid));
    found
}

fn calendar(s: &Session, id: Id) -> Calendar {
    s.mailbox.calendar(id).unwrap().clone()
}

#[test]
fn subscribing_makes_a_calendar_that_can_only_be_read() {
    let mut s = session();
    let id = subscribe(&mut s);
    let c = calendar(&s, id);
    assert_eq!((c.name.as_str(), c.rights, c.visible), ("Pessoal", Rights::Read, true));
    let sub = c.subscription.unwrap();
    assert_eq!(sub.url, URL);
    assert!(sub.stale && sub.last_refresh == 0 && sub.last_error.is_empty());
    // The same address, or the same name, twice is refused.
    assert!(s.run("calendar.subscribe", &json!({"url": URL, "name": "Other"})).is_err());
    assert!(s.run("calendar.subscribe", &json!({"url": "https://example.org/a.ics", "name": "pessoal"})).is_err());
    let color = s.run("calendar.subscribe", &json!({"url": "https://example.org/a.ics", "name": "F1", "color": "#2E8B57"})).unwrap();
    assert_eq!(calendar(&s, color["id"].as_u64().unwrap()).color, mailcraft_model::Rgb(0x2E, 0x8B, 0x57));
}

#[test]
fn an_address_the_policy_refuses_makes_no_calendar() {
    let mut s = session();
    let before = s.mailbox.calendars.len();
    for url in
        ["http://8.8.8.8/a.ics", "ftp://example.org/a.ics", "file:///etc/hosts", "https://user:pw@example.org/a.ics", "", "http://100.63.0.1/a.ics"]
    {
        assert!(s.run("calendar.subscribe", &json!({"url": url, "name": "X"})).is_err(), "{url}");
    }
    assert_eq!(s.mailbox.calendars.len(), before);
    for url in ["https://example.org/a.ics", "http://192.168.1.2/a.ics", "http://127.0.0.1:8241/a.ics"] {
        s.run("calendar.subscribe", &json!({"url": url, "name": url})).unwrap();
    }
}

#[test]
fn a_feed_with_an_all_day_event_and_a_timed_utc_event_is_read() {
    let mut s = session();
    let id = subscribe(&mut s);
    let done = apply(&mut s, id, &feed(&[("day@example.org", "Feriado", ALL_DAY), ("game@example.org", "Jogo", TIMED)]));
    assert_eq!((done["ok"].as_bool(), done["added"].as_u64()), (Some(true), Some(2)));
    let found = events_of(&s, id);
    let (day, game) = (&found[0], &found[1]);
    assert_eq!(game.uid, "game@example.org");
    assert_eq!((day.title.as_str(), day.all_day, day.location.as_str()), ("Feriado", true, "Sala 1"));
    assert_eq!((day.start.date.year, day.start.date.month, day.start.date.day), (2026, 11, 20));
    assert_eq!(day.description, "Linha um\nLinha dois");
    // The clock of the session is UTC: 20:00Z is 20:00.
    assert!(!game.all_day);
    assert_eq!((game.start.date.day, game.start.hour, game.start.minute), (21, 20, 0));
    assert_eq!((game.end.hour, game.end.minute), (21, 30));
    let sub = calendar(&s, id).subscription.unwrap();
    assert!(!sub.stale && sub.last_refresh == NOW && sub.last_attempt == NOW && sub.last_error.is_empty());
}

#[test]
fn the_next_feed_replaces_events_by_uid() {
    let mut s = session();
    let id = subscribe(&mut s);
    apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY), ("b@x", "B", TIMED), ("c@x", "C", LATER)]));
    let first = events_of(&s, id);
    let ids: Vec<Id> = first.iter().map(|e| e.id).collect();
    assert_eq!(first.len(), 3);
    // A is the same, B is renamed, C is gone, D is new.
    let done = apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY), ("b@x", "B renamed", TIMED), ("d@x", "D", LATER)]));
    assert_eq!(
        (done["added"].as_u64(), done["updated"].as_u64(), done["unchanged"].as_u64(), done["removed"].as_u64()),
        (Some(1), Some(1), Some(1), Some(1))
    );
    let now = events_of(&s, id);
    assert_eq!(now.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["A", "B renamed", "D"]);
    // Unchanged and changed events keep their ids; the removed one is gone; the new one has its own.
    assert_eq!((now[0].id, now[1].id), (ids[0], ids[1]));
    assert!(s.mailbox.event(ids[2]).is_none());
    assert!(!ids.contains(&now[2].id));
    // The same feed again changes nothing at all.
    let again = apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY), ("b@x", "B renamed", TIMED), ("d@x", "D", LATER)]));
    assert_eq!((again["added"].as_u64(), again["updated"].as_u64(), again["removed"].as_u64()), (Some(0), Some(0), Some(0)));
    assert_eq!(events_of(&s, id), now);
}

#[test]
fn the_replacement_touches_no_other_calendar() {
    let mut s = session();
    let before: Vec<Event> = s.mailbox.events.values().map(|e| (**e).clone()).collect();
    assert!(!before.is_empty());
    let id = subscribe(&mut s);
    apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY)]));
    apply(&mut s, id, &feed(&[]));
    assert!(events_of(&s, id).is_empty());
    let mut after: Vec<Event> = s.mailbox.events.values().filter(|e| e.calendar != id).map(|e| (**e).clone()).collect();
    let mut before = before;
    before.sort_by_key(|e| e.id);
    after.sort_by_key(|e| e.id);
    assert_eq!(before, after);
}

#[test]
fn a_feed_that_can_not_be_read_keeps_the_events_and_says_why() {
    let mut s = session();
    let id = subscribe(&mut s);
    apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY)]));
    let kept = events_of(&s, id);
    // Not a calendar at all (a login page), and an error the host reports.
    let page = apply(&mut s, id, "<html>log in</html>");
    assert_eq!(page["ok"].as_bool(), Some(false));
    assert_eq!(events_of(&s, id), kept);
    let sub = calendar(&s, id).subscription.unwrap();
    assert!(sub.last_error.contains("isn't an iCalendar file") && sub.last_attempt == NOW && !sub.stale && sub.last_refresh == NOW);
    let down = s.run("calendar.applyFeed", &json!({"id": id, "error": "the server\ncouldn't be reached\u{7}"})).unwrap();
    assert_eq!(down["error"].as_str(), Some("the server couldn't be reached"));
    assert_eq!(events_of(&s, id), kept);
    assert_eq!(calendar(&s, id).subscription.unwrap().last_error, "the server couldn't be reached");
    // The next good read clears it.
    apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY)]));
    assert!(calendar(&s, id).subscription.unwrap().last_error.is_empty());
    // A feed is for a calendar that is one, and says one thing or the other.
    let own = s.mailbox.calendars.iter().find(|c| c.subscription.is_none()).unwrap().id;
    assert!(s.run("calendar.applyFeed", &json!({"id": own, "ics": feed(&[])})).is_err());
    assert!(s.run("calendar.applyFeed", &json!({"id": id})).is_err());
}

#[test]
fn refreshing_marks_every_feed_and_the_schedule_waits_three_hours() {
    let mut s = session();
    let id = subscribe(&mut s);
    apply(&mut s, id, &feed(&[]));
    let sub = calendar(&s, id).subscription.unwrap();
    assert!(!sub.due(NOW + 3 * 3600 - 1) && sub.due(NOW + 3 * 3600));
    let marked = s.run("calendar.refreshSubscriptions", &json!({})).unwrap();
    assert_eq!(marked["subscriptions"].as_u64(), Some(1));
    assert!(calendar(&s, id).subscription.unwrap().due(NOW));
}

#[test]
fn what_comes_from_an_address_is_not_changed_here() {
    let mut s = session();
    let id = subscribe(&mut s);
    apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY)]));
    let event = events_of(&s, id)[0].id;
    for (command, params) in [
        ("calendar.edit", json!({"id": event, "title": "Mine"})),
        ("calendar.move", json!({"id": event, "start": "2026-11-25T10:00"})),
        ("calendar.delete", json!({"id": event})),
        ("calendar.showAs", json!({"id": event, "value": "free"})),
        ("calendar.reminder", json!({"id": event, "minutes": 5})),
        ("calendar.private", json!({"id": event, "value": true})),
        ("calendar.categorize", json!({"id": event, "category": "Red"})),
        ("calendar.setCurrent", json!({"id": id})),
        ("calendar.new", json!({"title": "Into the feed", "calendar": id})),
    ] {
        assert!(s.run(command, &params).is_err(), "{command}");
    }
    assert_eq!(events_of(&s, id).len(), 1);
    assert_eq!(events_of(&s, id)[0].title, "A");
    // New events never go into it by themselves: not even with it the only one on this computer.
    let made = s.run("calendar.new", &json!({"title": "Mine"})).unwrap();
    let made = s.mailbox.event(made["id"].as_u64().unwrap()).unwrap();
    assert_ne!(made.calendar, id);
}

#[test]
fn unsubscribing_takes_the_calendar_and_its_events_and_nothing_else() {
    let mut s = session();
    let id = subscribe(&mut s);
    apply(&mut s, id, &feed(&[("a@x", "A", ALL_DAY), ("b@x", "B", TIMED)]));
    let own = s.mailbox.calendars.iter().find(|c| c.subscription.is_none()).unwrap().id;
    assert!(s.run("calendar.unsubscribe", &json!({"id": own})).is_err());
    assert!(s.run("calendar.unsubscribe", &json!({"id": 999_999})).is_err());
    let done = s.run("calendar.unsubscribe", &json!({"id": id})).unwrap();
    assert_eq!(done["events"].as_u64(), Some(2));
    assert!(s.mailbox.calendar(id).is_none() && s.mailbox.calendar(own).is_some());
    assert!(events_of(&s, id).is_empty());
}

#[test]
fn an_agent_may_subscribe_and_refresh_but_removing_one_needs_the_owner() {
    assert_eq!(access("calendar.subscribe"), Some(Access::Safe));
    assert_eq!(access("calendar.refreshSubscriptions"), Some(Access::Safe));
    assert_eq!(access("calendar.applyFeed"), Some(Access::Safe));
    assert_eq!(access("calendar.unsubscribe"), Some(Access::Owner(Class::Delete)));
}
