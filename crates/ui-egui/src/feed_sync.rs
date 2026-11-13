// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Internet calendars in the window: a calendar subscribed to an `.ics` address
//! (`calendar.subscribe`, [`mailcraft_engine::cmd::calendar_feed`]) is read again when it is due.
//!
//! What is due is the mailbox's to say ([`mailcraft_model::Subscription::due`]): a calendar just
//! subscribed to, one that Send/Receive or an agent marked (`calendar.refreshSubscriptions`), or
//! one not tried for three hours. Every frame looks, and starts at most one read at a time
//! through the host ([`Services::feed_fetch`](crate::Services::feed_fetch), which asks the
//! address on a background thread). What comes back goes to `calendar.applyFeed`, which replaces
//! the calendar's events or keeps them and notes why not. A read that fails is waited out as
//! long as one that worked, and is never a dialog: one line in the log (without the address,
//! which may hold a secret) and, for a calendar that never worked, the status bar.
//!
//! Work Offline stops the reads; so does removing MailCraft's data.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::MailApp;

/// How soon the same calendar may be asked for again whatever the mailbox says: a read that
/// can't be written down (the calendar went, the mailbox refused) must not repeat every frame.
const MIN_GAP: Duration = Duration::from_secs(60);
/// How often an idle window looks whether a feed is due.
const LOOK_EVERY: Duration = Duration::from_secs(300);

/// The window's reads of internet calendars: the handle only.
#[derive(Default)]
pub(crate) struct FeedSync {
    /// The read under way: the calendar, and the answer to come.
    rx: Option<(Id, Receiver<Result<String, String>>)>,
    /// When each calendar was last asked for, in this run.
    asked: HashMap<Id, Instant>,
}

#[cfg(test)]
impl FeedSync {
    /// Forget when each calendar was asked for (the tests' minute is a click away).
    pub(crate) fn clear_gap(&mut self) {
        self.asked.clear();
    }
}

impl MailApp {
    /// Every frame: take in the answer of a read, and start the next one that is due.
    pub(crate) fn feeds_tick(&mut self) {
        if let Some((id, rx)) = self.feeds.rx.take() {
            match rx.try_recv() {
                Ok(answer) => self.finish_feed(id, answer),
                Err(TryRecvError::Empty) => {
                    self.feeds.rx = Some((id, rx));
                    return;
                }
                Err(TryRecvError::Disconnected) => self.finish_feed(id, Err(tl!("the read stopped").to_string())),
            }
        }
        if self.data_removed || self.offline() || self.opening.is_some() {
            return;
        }
        let Some(fetch) = &self.services.feed_fetch else { return };
        let now = self.session.now();
        let mut any = false;
        let mut next: Option<(Id, String)> = None;
        for c in &self.session.mailbox.calendars {
            let Some(feed) = &c.subscription else { continue };
            any = true;
            let recent = self.feeds.asked.get(&c.id).is_some_and(|at| at.elapsed() < MIN_GAP);
            if next.is_none() && !recent && feed.due(now) {
                next = Some((c.id, feed.url.clone()));
            }
        }
        if let Some(ctx) = &self.ctx {
            if next.is_some() {
                ctx.request_repaint();
            } else if any {
                ctx.request_repaint_after(LOOK_EVERY);
            }
        }
        if let Some((id, url)) = next {
            self.feeds.asked.insert(id, Instant::now());
            self.feeds.rx = Some((id, fetch(url)));
        }
    }

    /// What the host read from calendar `id`'s address (or why it couldn't) goes into the mailbox.
    fn finish_feed(&mut self, id: Id, answer: Result<String, String>) {
        let never_worked = self.session.mailbox.calendar(id).and_then(|c| c.subscription.as_ref()).is_some_and(|s| s.last_refresh == 0);
        let params = match &answer {
            Ok(ics) => json!({"id": id, "ics": ics}),
            Err(why) => json!({"id": id, "error": why}),
        };
        let said = match self.execute("calendar.applyFeed", params) {
            Ok(done) if done.get("ok").and_then(Value::as_bool) == Some(true) => return,
            Ok(done) => done.get("error").and_then(Value::as_str).unwrap_or_default().to_string(),
            // (The calendar went while it was being read, or the mailbox said no.)
            Err(e) => e,
        };
        log::info!("internet calendar {id} couldn't be updated: {said}");
        if never_worked {
            self.status(crate::i18n::fmt(tl!("An internet calendar couldn't be updated: {why}"), &[("why", &said)]));
        }
    }
}

/// What the window does for the owner's Send/Receive: every internet calendar is read again.
pub(crate) fn send_receive_asked(app: &mut MailApp) {
    if app.session.mailbox.calendars.iter().any(|c| c.subscription.is_some()) {
        let _ = app.execute("calendar.refreshSubscriptions", json!({}));
    }
}
