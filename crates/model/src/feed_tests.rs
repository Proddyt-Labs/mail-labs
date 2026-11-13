// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Internet calendars: which addresses may be subscribed to, when a feed is due, and the file.

use serde_json::json;

use crate::*;

#[test]
fn https_is_taken_anywhere() {
    for url in ["https://example.org/a.ics", "HTTPS://203.0.113.9:8443/x?y=1", "https://[2001:db8::1]/c.ics"] {
        assert_eq!(check_feed_url(url), Ok(()), "{url}");
    }
}

#[test]
fn plain_http_is_only_for_private_addresses() {
    let allowed = [
        "http://10.1.2.3/a.ics",
        "http://172.16.0.1/a.ics",
        "http://172.31.255.255/a.ics",
        "http://192.168.1.5:8080/a.ics",
        "http://127.0.0.1:8241/s/a.ics",
        "http://100.87.104.117:8241/secret/google/pessoal.ics",
        "http://100.64.0.1/a.ics",
        "http://100.127.255.254/a.ics",
        "http://[::1]/a.ics",
        "http://[fd00::1]/a.ics",
        "http://[::ffff:192.168.0.1]/a.ics",
    ];
    for url in allowed {
        assert_eq!(check_feed_url(url), Ok(()), "{url}");
    }
    let refused = [
        "http://8.8.8.8/a.ics",
        "http://100.63.255.255/a.ics",
        "http://100.128.0.1/a.ics",
        "http://172.15.0.1/a.ics",
        "http://172.32.0.1/a.ics",
        "http://192.169.0.1/a.ics",
        "http://11.0.0.1/a.ics",
        "http://[2001:db8::1]/a.ics",
        "http://[::ffff:8.8.8.8]/a.ics",
    ];
    for url in refused {
        assert_eq!(check_feed_url(url), Err(FeedUrlError::PlainHttp), "{url}");
    }
}

#[test]
fn what_is_not_a_web_address_is_refused() {
    for url in
        ["", "   ", "ftp://example.org/a.ics", "file:///etc/passwd", "example.org/a.ics", "https://", "http://:80/x", "https://a b/c", "webcal://x/y"]
    {
        assert_eq!(check_feed_url(url), Err(FeedUrlError::NotAddress), "{url:?}");
    }
    assert_eq!(check_feed_url("https://user:pw@example.org/a.ics"), Err(FeedUrlError::Credentials));
    assert_eq!(check_feed_url("http://10.0.0.1@8.8.8.8/a.ics"), Err(FeedUrlError::Credentials));
    assert_eq!(check_feed_url(&format!("https://example.org/{}", "a".repeat(MAX_FEED_URL))), Err(FeedUrlError::NotAddress));
}

#[test]
fn a_feed_is_due_when_asked_never_tried_or_old() {
    let at = 1_000_000;
    let mut s = Subscription { url: "https://example.org/a.ics".into(), last_attempt: at, ..Default::default() };
    assert!(!s.due(at + FEED_EVERY_SECS - 1));
    assert!(s.due(at + FEED_EVERY_SECS));
    s.stale = true;
    assert!(s.due(at));
    assert!(Subscription::default().due(at));
}

#[test]
fn a_subscription_survives_a_save_and_a_hostile_one_reads_as_none() {
    let sub = Subscription { url: "https://example.org/a.ics".into(), last_refresh: 5, last_attempt: 6, last_error: "boom".into(), stale: true };
    let c = Calendar { id: 4, name: "Feed".into(), rights: Rights::Read, subscription: Some(sub.clone()), ..Default::default() };
    let back: Calendar = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
    assert_eq!(back.subscription, Some(sub));
    assert!(serde_json::to_value(Calendar::default()).unwrap().get("subscription").is_none());
    let junk: Calendar = serde_json::from_value(json!({"id": 1, "subscription": {"url": 7, "stale": "x"}})).unwrap();
    assert_eq!(junk.subscription, None);
}
