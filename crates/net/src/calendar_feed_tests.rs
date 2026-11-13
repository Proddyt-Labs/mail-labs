// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading an internet calendar from a stand-in web server on this machine.

use crate::calendar_feed::{FeedError, fetch};
use crate::fake::FakeHttp;
use mailcraft_model::FeedUrlError;

const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn answer(status: &str, body: &[u8]) -> Vec<u8> {
    let mut out =
        format!("HTTP/1.1 {status}\r\nContent-Type: text/calendar; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
            .into_bytes();
    out.extend_from_slice(body);
    out
}

#[test]
fn a_feed_on_this_machine_is_read_and_asked_for_plainly() {
    let server = FakeHttp::answering(answer("200 OK", FEED.as_bytes()));
    let text = fetch(&server.url("/secret/google/pessoal.ics?x=1")).unwrap();
    assert_eq!(text, FEED);
    let request = server.request(0).to_ascii_lowercase();
    assert!(request.starts_with("get /secret/google/pessoal.ics?x=1 http/1.1\r\n"), "{request}");
    assert!(request.contains("accept: text/calendar"));
    // Nothing that says who is asking.
    assert!(!request.contains("cookie") && !request.contains("authorization") && !request.contains("referer"));
}

#[test]
fn a_byte_order_mark_is_dropped() {
    let mut body = vec![0xEF, 0xBB, 0xBF];
    body.extend_from_slice(FEED.as_bytes());
    let server = FakeHttp::answering(answer("200 OK", &body));
    assert_eq!(fetch(&server.url("/a.ics")).unwrap(), FEED);
}

#[test]
fn what_is_not_a_calendar_a_status_or_a_redirect_is_an_error_that_names_no_address() {
    let page = FakeHttp::answering(answer("200 OK", b"<html>login</html>"));
    assert_eq!(fetch(&page.url("/a.ics")), Err(FeedError::NotCalendar));
    let missing = FakeHttp::answering(answer("404 Not Found", b"no"));
    assert_eq!(fetch(&missing.url("/a.ics")), Err(FeedError::Status(404)));
    let moved =
        FakeHttp::answering(b"HTTP/1.1 301 Moved\r\nLocation: https://example.org/b.ics\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
    let error = fetch(&moved.url("/s3cret/a.ics")).unwrap_err();
    assert_eq!(error, FeedError::Redirect);
    for e in [error, FeedError::Status(500), FeedError::NotCalendar, FeedError::Unreachable] {
        assert!(!e.to_string().contains("s3cret") && !e.to_string().contains("127.0.0.1"));
    }
}

#[test]
fn a_feed_that_says_it_is_too_long_is_refused() {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\nBEGIN:VCALENDAR", 11 * 1024 * 1024);
    let server = FakeHttp::answering(head.into_bytes());
    assert_eq!(fetch(&server.url("/a.ics")), Err(FeedError::TooLarge));
}

#[test]
fn the_policy_is_kept_before_anything_is_asked() {
    assert_eq!(fetch("http://8.8.8.8/a.ics"), Err(FeedError::Address(FeedUrlError::PlainHttp)));
    assert_eq!(fetch("ftp://127.0.0.1/a.ics"), Err(FeedError::Address(FeedUrlError::NotAddress)));
    assert_eq!(fetch("http://user@127.0.0.1/a.ics"), Err(FeedError::Address(FeedUrlError::Credentials)));
}

#[test]
fn nothing_listening_is_unreachable() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    assert_eq!(fetch(&format!("http://127.0.0.1:{port}/a.ics")), Err(FeedError::Unreachable));
}
