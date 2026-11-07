//! Full wire path through the public API: the desktop's pairing QR JSON is what the phone
//! scans, encrypts with, and the desktop then accepts. Also proves integration test binaries
//! link Tauri (build.rs manifest, tauri#13419).

use otp_forwarder_desktop_lib::crypto;
use otp_forwarder_desktop_lib::otp::{self, Dedupe, Reject};
use otp_forwarder_desktop_lib::pairing::{DEFAULT_RELAY, Pairing};

const NOW: i64 = 1_700_000_000_000;

/// What the phone does after scanning the QR code.
fn phone_send(qr_json: &str, code: &str, ts: i64) -> (String, String) {
    let pairing = Pairing::from_json(qr_json).unwrap();
    let id = otp::new_id().unwrap();
    let json = otp::encode_plaintext(&id, code, "+41000000000", ts);
    let body = crypto::encrypt(&pairing.key, &pairing.topic, json.as_bytes()).unwrap();
    (id, body)
}

#[test]
fn phone_message_from_scanned_pairing_is_accepted_once() {
    let desktop = Pairing::generate(DEFAULT_RELAY).unwrap();
    let qr_json = desktop.to_json().unwrap();
    let (id, body) = phone_send(&qr_json, "012345", NOW - 1_000);

    let mut seen = Dedupe::default();
    let msg = otp::receive(&desktop.key, &desktop.topic, &body, NOW, &mut seen).unwrap();
    assert_eq!(msg.id, id);
    assert_eq!(msg.code, "012345");
    assert_eq!(msg.sender, "+41000000000");
    assert_eq!(
        otp::receive(&desktop.key, &desktop.topic, &body, NOW, &mut seen).err(),
        Some(Reject::Duplicate)
    );

    // After re-pairing, messages from the old pairing are rejected.
    let repaired = Pairing::generate(DEFAULT_RELAY).unwrap();
    let (_, old) = phone_send(&qr_json, "999999", NOW);
    assert_eq!(
        otp::receive(&repaired.key, &repaired.topic, &old, NOW, &mut seen).err(),
        Some(Reject::Undecryptable)
    );
}
