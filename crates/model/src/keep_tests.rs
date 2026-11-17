// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Proddyt Labs fork: what the Google Keep types show of a sign-in, and what the mailbox keeps of
//! Keep (nothing of a sign-in). Every token here is made up.

use crate::keep::{KeepState, Secret};
use crate::{Account, AccountKind, Mailbox};

const MASTER: &str = "aas_et/STANDIN-MASTER-0001";

#[test]
fn a_secret_shows_nothing_in_debug_or_display() {
    let secret = Secret::new(MASTER.to_string());
    for shown in [format!("{secret:?}"), format!("{secret:#?}"), format!("{secret}"), format!("{:?}", Some(secret.clone()))] {
        assert!(!shown.contains("STANDIN") && !shown.contains(MASTER), "{shown}");
    }
    // The owner's code can still reach it, where it is needed.
    assert_eq!(secret.expose(), MASTER);
    assert!(!secret.is_empty() && Secret::new(String::new()).is_empty());
}

#[test]
fn a_secret_in_a_struct_keeps_the_struct_debug_clean() {
    #[derive(Debug)]
    #[allow(dead_code)]
    struct Holder {
        name: &'static str,
        master: Secret,
    }
    let shown = format!("{:?}", Holder { name: "owner", master: Secret::new(MASTER.to_string()) });
    assert!(shown.contains("owner") && !shown.contains("STANDIN"), "{shown}");
}

#[test]
fn the_mailbox_keeps_keeps_version_and_labels_never_a_sign_in() {
    let mut mailbox = Mailbox::default();
    let account =
        mailbox.add_account(Account { email: "owner@example.org".into(), kind: AccountKind::Imap, google_keep: true, ..Default::default() });
    mailbox.keep.insert(account, KeepState { version: "v9".into(), labels: Vec::new() });
    let saved = serde_json::to_string(&mailbox).unwrap();
    assert!(saved.contains("\"version\":\"v9\""), "the version is kept");
    assert!(!saved.contains("aas_et") && !saved.contains("oauth2_4") && !saved.contains("ya29."), "a sign-in is never kept in the mailbox");
}
