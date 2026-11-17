// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: the Google Keep sign-in ([`crate::keep_auth`]), against a stand-in for
//! Google's Android sign-in server. Nothing here goes to the network, and every token in it is
//! made up.

use std::cell::RefCell;

use crate::NetError;
use crate::http::Url;
use crate::keep_auth::{
    KEEP_APP, KEEP_CLIENT_SIG, KEEP_SCOPES, Secret, access_token_with, android_id, exchange_token_with, may_go_to,
};

const COOKIE: &str = "oauth2_4/standin-cookie-value-123";
const MASTER: &str = "aas_et/STANDIN-MASTER-TOKEN-456";
const ACCESS: &str = "ya29.standin-access-token-789";
const EMAIL: &str = "someone@example.org";

type Sent = RefCell<Vec<Vec<(String, String)>>>;

/// A stand-in that records the form it was sent and answers `answer`.
fn stand_in<'a>(sent: &'a Sent, status: u16, answer: &'a str) -> impl Fn(&[(&str, &str)]) -> Result<(u16, Vec<u8>), NetError> + 'a {
    move |fields: &[(&str, &str)]| {
        sent.borrow_mut().push(fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        Ok((status, answer.as_bytes().to_vec()))
    }
}

fn field(sent: &Sent, name: &str) -> Option<String> {
    sent.borrow().last()?.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

#[test]
fn the_cookie_is_traded_for_the_master_token_the_way_the_android_app_asks() {
    let sent = Sent::default();
    let fetch = stand_in(&sent, 200, "Auth=short\nEmail=someone@example.org\nToken=aas_et/STANDIN-MASTER-TOKEN-456\nSID=ignored\n");
    let mut cookie = COOKIE.to_string();
    let master = exchange_token_with(&fetch, EMAIL, &mut cookie).unwrap();
    assert_eq!(master.expose(), MASTER);
    for (name, value) in [
        ("ACCESS_TOKEN", "1"),
        ("add_account", "1"),
        ("service", "ac2dm"),
        ("source", "android"),
        ("accountType", "HOSTED_OR_GOOGLE"),
        ("Email", EMAIL),
        ("Token", COOKIE),
        ("client_sig", KEEP_CLIENT_SIG),
        ("androidId", android_id(EMAIL).as_str()),
    ] {
        assert_eq!(field(&sent, name).as_deref(), Some(value), "{name}");
    }
    // (The Android id is a fixed 16 hex digits, the same each time, and says nothing of the address.)
    assert_eq!(android_id(EMAIL).len(), 16);
    assert_eq!(android_id(EMAIL), android_id(" Someone@Example.org "));
    assert!(!android_id(EMAIL).contains("someone"));
}

#[test]
fn the_cookie_buffer_is_overwritten_whatever_came_of_the_trade() {
    for (status, answer) in [(200, "Token=aas_et/x\n"), (200, "Error=BadAuthentication\n"), (500, "")] {
        let sent = Sent::default();
        let fetch = stand_in(&sent, status, answer);
        let mut cookie = COOKIE.to_string();
        let _ = exchange_token_with(&fetch, EMAIL, &mut cookie);
        assert!(cookie.is_empty(), "the cookie is still there after a {status}");
    }
    // Not even a cookie that is refused unread stays.
    let mut bad = "has a space".to_string();
    let _ = exchange_token_with(&stand_in(&Sent::default(), 200, ""), EMAIL, &mut bad);
    assert!(bad.is_empty());
}

#[test]
fn the_access_token_is_asked_for_as_the_keep_app_with_the_master_token() {
    let sent = Sent::default();
    let fetch = stand_in(&sent, 200, "Auth=ya29.standin-access-token-789\nExpiry=123\n");
    let access = access_token_with(&fetch, EMAIL, &Secret::new(MASTER.into())).unwrap();
    assert_eq!(access.expose(), ACCESS);
    assert_eq!(field(&sent, "EncryptedPasswd").as_deref(), Some(MASTER));
    assert_eq!(field(&sent, "service").as_deref(), Some(KEEP_SCOPES));
    assert!(KEEP_SCOPES.contains("auth/memento") && KEEP_SCOPES.contains("auth/reminders"));
    assert_eq!(field(&sent, "app").as_deref(), Some(KEEP_APP));
    assert_eq!(field(&sent, "client_sig").as_deref(), Some("38918a453d07199354f8b19af05ec6562ced5788"));
    assert_eq!(field(&sent, "androidId"), Some(android_id(EMAIL)));
}

#[test]
fn a_refusal_says_a_fixed_sentence_and_never_what_google_wrote() {
    // Google's answer carries a token and the error code, as it does when it half-works.
    let answers = [
        (200, "Error=BadAuthentication\nToken=aas_et/LEAKED-IN-ERROR\nUrl=https://example.org/leak\n"),
        (200, "Error=NeedsBrowser\nUrl=https://example.org/leak?token=aas_et/LEAKED-IN-ERROR\n"),
        (403, "Error=Whatever aas_et/LEAKED-IN-ERROR"),
        (200, "garbage aas_et/LEAKED-IN-ERROR"),
        (404, "aas_et/LEAKED-IN-ERROR"),
        (500, "aas_et/LEAKED-IN-ERROR"),
        (429, "aas_et/LEAKED-IN-ERROR"),
    ];
    for (status, answer) in answers {
        let sent = Sent::default();
        let fetch = stand_in(&sent, status, answer);
        let mut cookie = COOKIE.to_string();
        let error = exchange_token_with(&fetch, EMAIL, &mut cookie).unwrap_err();
        let shown = format!("{error} / {error:?}");
        assert!(!shown.contains("LEAKED"), "{status}: {shown}");
        assert!(!shown.contains("example.org/leak"), "{status}: {shown}");
        assert!(!shown.contains(COOKIE), "{status}: {shown}");
        // The access request, the same.
        let error = access_token_with(&fetch, EMAIL, &Secret::new(MASTER.into())).unwrap_err();
        let shown = format!("{error} / {error:?}");
        assert!(!shown.contains("LEAKED") && !shown.contains(MASTER), "{status}: {shown}");
    }
    // A refused sign-in is the kind the window answers by asking to connect again; a busy server is not.
    let refused = exchange_token_with(&stand_in(&Sent::default(), 200, "Error=BadAuthentication\n"), EMAIL, &mut COOKIE.to_string()).unwrap_err();
    assert!(refused.is_refused_sign_in());
    let busy = exchange_token_with(&stand_in(&Sent::default(), 503, ""), EMAIL, &mut COOKIE.to_string()).unwrap_err();
    assert!(!busy.is_refused_sign_in());
}

#[test]
fn an_answer_without_the_token_is_no_success_even_with_status_200() {
    let error = exchange_token_with(&stand_in(&Sent::default(), 200, "Auth=only-this\nEmail=x\n"), EMAIL, &mut COOKIE.to_string()).unwrap_err();
    assert!(error.is_refused_sign_in());
    // A token that a header or a form can't carry isn't one.
    for odd in ["Token=has space in it\n", "Token=\n", "Token=new\u{1}line\n"] {
        assert!(exchange_token_with(&stand_in(&Sent::default(), 200, odd), EMAIL, &mut COOKIE.to_string()).is_err(), "{odd:?}");
    }
}

#[test]
fn a_secret_shows_itself_nowhere() {
    let secret = Secret::new(MASTER.to_string());
    for shown in [format!("{secret:?}"), format!("{secret}"), format!("{secret:#?}"), format!("{:?}", Some(&secret)), format!("{:?}", vec![secret.clone()])] {
        assert!(!shown.contains("aas_et") && !shown.contains("STANDIN"), "{shown}");
    }
    assert_eq!(secret.expose(), MASTER);
    assert!(!secret.is_empty());
}

#[test]
fn the_master_token_goes_to_googles_android_sign_in_server_alone() {
    assert!(may_go_to(&Url::parse("https://android.clients.google.com/auth").unwrap()).is_ok());
    for elsewhere in [
        "https://www.googleapis.com/auth",
        "https://android.clients.google.com.example.org/auth",
        "https://example.org/auth",
        "http://127.0.0.1/auth",
        "https://android.clients.google.com:8443/auth",
    ] {
        assert!(may_go_to(&Url::parse(elsewhere).unwrap()).is_err(), "{elsewhere}");
    }
}

#[test]
fn an_address_or_a_secret_that_is_none_is_refused_before_anything_is_sent() {
    let sent = Sent::default();
    let fetch = stand_in(&sent, 200, "Token=aas_et/x\n");
    assert!(exchange_token_with(&fetch, "", &mut COOKIE.to_string()).is_err());
    assert!(exchange_token_with(&fetch, "two words@example.org", &mut COOKIE.to_string()).is_err());
    assert!(exchange_token_with(&fetch, EMAIL, &mut String::new()).is_err());
    assert!(access_token_with(&fetch, EMAIL, &Secret::new(String::new())).is_err());
    assert!(sent.borrow().is_empty());
}
