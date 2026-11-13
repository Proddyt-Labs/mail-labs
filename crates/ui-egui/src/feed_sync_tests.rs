// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Internet calendars in the window: a subscription is read once, what comes back is applied, a
//! failure is quiet and waited out, and Work Offline reads nothing.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
const URL: &str = "http://192.168.1.10:8241/secret/f1.ics";
const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:gp@example.org\r\nDTSTART:20261122T130000Z\r\nDTEND:20261122T150000Z\r\nSUMMARY:Grand Prix\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// A host whose reads answer what `answer` holds, and are counted with the addresses asked.
fn host(answer: Result<String, String>) -> (Arc<Mutex<Result<String, String>>>, Arc<AtomicUsize>, MailApp) {
    let answer = Arc::new(Mutex::new(answer));
    let asked = Arc::new(AtomicUsize::new(0));
    let (held, count) = (answer.clone(), asked.clone());
    let services = Services {
        feed_fetch: Some(Box::new(move |url| {
            assert_eq!(url, URL);
            count.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(held.lock().unwrap().clone());
            rx
        })),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), services);
    app.autosave = false;
    (answer, asked, app)
}

fn subscribe(app: &mut MailApp) -> u64 {
    app.execute("calendar.subscribe", json!({"url": URL, "name": "F1"})).unwrap()["id"].as_u64().unwrap()
}

/// Two frames: the read starts, then its answer is taken in.
fn frames(app: &mut MailApp) {
    app.feeds_tick();
    app.feeds_tick();
}

fn titles(app: &MailApp, calendar: u64) -> Vec<String> {
    app.session.mailbox.events.values().filter(|e| e.calendar == calendar).map(|e| e.title.clone()).collect()
}

#[test]
fn a_new_subscription_is_read_once_and_its_events_come_in() {
    let (_, asked, mut app) = host(Ok(FEED.to_string()));
    let id = subscribe(&mut app);
    frames(&mut app);
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    assert_eq!(titles(&app, id), ["Grand Prix"]);
    // Nothing is due for three hours: no more reads.
    for _ in 0..5 {
        frames(&mut app);
    }
    assert_eq!(asked.load(Ordering::Relaxed), 1);
}

#[test]
fn a_read_that_fails_is_quiet_keeps_the_events_and_is_waited_out() {
    let (answer, asked, mut app) = host(Ok(FEED.to_string()));
    let id = subscribe(&mut app);
    frames(&mut app);
    // Send/Receive asks again; the stand-in's server is down now.
    *answer.lock().unwrap() = Err("the server couldn't be reached".into());
    app.feeds.clear_gap();
    crate::feed_sync::send_receive_asked(&mut app);
    frames(&mut app);
    assert_eq!(asked.load(Ordering::Relaxed), 2);
    assert_eq!(titles(&app, id), ["Grand Prix"]);
    let feed = app.session.mailbox.calendar(id).unwrap().subscription.clone().unwrap();
    assert_eq!(feed.last_error, "the server couldn't be reached");
    assert!(!feed.stale);
    // No dialog, and no loop: the failure is waited out like a success.
    assert!(app.dialog.is_none());
    app.feeds.clear_gap();
    for _ in 0..5 {
        frames(&mut app);
    }
    assert_eq!(asked.load(Ordering::Relaxed), 2);
}

#[test]
fn a_subscription_that_never_worked_says_so_in_the_status_bar() {
    let (_, _, mut app) = host(Err("the server answered 404".into()));
    let id = subscribe(&mut app);
    frames(&mut app);
    assert!(titles(&app, id).is_empty());
    let said = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(said.contains("the server answered 404"), "{said}");
}

#[test]
fn working_offline_reads_nothing() {
    let (_, asked, mut app) = host(Ok(FEED.to_string()));
    subscribe(&mut app);
    app.session.prefs.offline = true;
    frames(&mut app);
    assert_eq!(asked.load(Ordering::Relaxed), 0);
    app.session.prefs.offline = false;
    frames(&mut app);
    assert_eq!(asked.load(Ordering::Relaxed), 1);
}
