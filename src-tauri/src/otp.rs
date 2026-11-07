//! Plaintext OTP message model and the desktop acceptance rules (wire contract v1).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::crypto::{self, CryptoError, Key};

pub const VERSION: u8 = 1;
pub const MAX_PLAINTEXT_BYTES: usize = 512;
/// Older than this (phone clock vs. ours) is stale.
pub const MAX_AGE_MS: i64 = 120_000;
/// Further in the future than this is rejected.
pub const MAX_FUTURE_MS: i64 = 30_000;
/// How long an accepted message id blocks replays.
pub const DEDUPE_TTL_MS: i64 = 600_000;
const CODE_MIN_LEN: usize = 4;
const CODE_MAX_LEN: usize = 10;
const MAX_SENDER_CHARS: usize = 64;

/// Decrypted, validated message. No `Debug`: it carries the code, which is zeroized on drop.
#[derive(Deserialize)]
pub struct OtpMessage {
    pub v: u8,
    pub id: String,
    pub code: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub sender: String,
    /// Unix ms, phone clock.
    pub ts: i64,
}

impl Drop for OtpMessage {
    fn drop(&mut self) {
        self.code.zeroize();
    }
}

/// Why a message was rejected. Never carries the code or the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reject {
    #[error("message body too large")]
    TooLarge,
    #[error("message could not be decrypted (wrong key/topic or tampered)")]
    Undecryptable,
    #[error("message is not valid JSON of the expected shape")]
    Malformed,
    #[error("unsupported message version {v}")]
    UnsupportedVersion { v: u8 },
    #[error("invalid message id")]
    InvalidId,
    #[error("invalid code format")]
    InvalidCode,
    #[error("invalid sender")]
    InvalidSender,
    #[error("message is stale ({age_s} s old)")]
    Stale { age_s: i64 },
    #[error("message timestamp is {ahead_s} s in the future")]
    FromFuture { ahead_s: i64 },
    #[error("message was already received")]
    Duplicate,
}

/// Ids of accepted messages with their expiry (unix ms); pruned on every check.
#[derive(Default)]
pub struct Dedupe(HashMap<String, i64>);

impl Dedupe {
    fn record(&mut self, id: &str, now_ms: i64) -> Result<(), Reject> {
        self.0.retain(|_, expires_ms| *expires_ms > now_ms);
        if self.0.contains_key(id) {
            return Err(Reject::Duplicate);
        }
        self.0
            .insert(id.to_owned(), now_ms.saturating_add(DEDUPE_TTL_MS));
        Ok(())
    }
}

/// Decrypts and validates a relay message body, in contract order. Only fully valid
/// messages are recorded in `seen`.
pub fn receive(
    key: &Key,
    topic: &str,
    body: &str,
    now_ms: i64,
    seen: &mut Dedupe,
) -> Result<OtpMessage, Reject> {
    let plaintext = crypto::decrypt(key, topic, body).map_err(|e| match e {
        CryptoError::TooLarge => Reject::TooLarge,
        CryptoError::Rng(_) | CryptoError::Malformed | CryptoError::Auth => Reject::Undecryptable,
    })?;
    let msg = parse(&plaintext)?;
    msg.validate(now_ms)?;
    seen.record(&msg.id, now_ms)?;
    Ok(msg)
}

/// New message id: base64url of 16 random bytes.
pub fn new_id() -> Result<String, CryptoError> {
    crypto::generate_topic()
}

/// Plaintext JSON for a message, as the phone sends it (for the CLI and test codes).
pub fn encode_plaintext(id: &str, code: &str, sender: &str, ts: i64) -> Zeroizing<String> {
    #[derive(Serialize)]
    struct Plaintext<'a> {
        v: u8,
        id: &'a str,
        code: &'a str,
        sender: &'a str,
        ts: i64,
    }
    let plaintext = Plaintext {
        v: VERSION,
        id,
        code,
        sender,
        ts,
    };
    // Serializing strings and integers cannot fail.
    Zeroizing::new(serde_json::to_string(&plaintext).unwrap_or_default())
}

fn parse(plaintext: &[u8]) -> Result<OtpMessage, Reject> {
    if plaintext.len() > MAX_PLAINTEXT_BYTES {
        return Err(Reject::TooLarge);
    }
    serde_json::from_slice(plaintext).map_err(|_| Reject::Malformed)
}

impl OtpMessage {
    fn validate(&self, now_ms: i64) -> Result<(), Reject> {
        if self.v != VERSION {
            return Err(Reject::UnsupportedVersion { v: self.v });
        }
        if !crypto::is_valid_token(&self.id) {
            return Err(Reject::InvalidId);
        }
        if !is_valid_code(&self.code) {
            return Err(Reject::InvalidCode);
        }
        if !is_valid_sender(&self.sender) {
            return Err(Reject::InvalidSender);
        }
        check_freshness(self.ts, now_ms)
    }
}

/// `^[A-Za-z0-9]{4,10}$`
pub fn is_valid_code(code: &str) -> bool {
    (CODE_MIN_LEN..=CODE_MAX_LEN).contains(&code.len())
        && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

// Kotlin serializers emit `null` for absent optional strings.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

fn is_valid_sender(sender: &str) -> bool {
    sender.chars().count() <= MAX_SENDER_CHARS && !sender.chars().any(char::is_control)
}

fn check_freshness(ts_ms: i64, now_ms: i64) -> Result<(), Reject> {
    let age_ms = now_ms.saturating_sub(ts_ms);
    if age_ms > MAX_AGE_MS {
        return Err(Reject::Stale {
            age_s: age_ms / 1000,
        });
    }
    let ahead_ms = ts_ms.saturating_sub(now_ms);
    if ahead_ms > MAX_FUTURE_MS {
        return Err(Reject::FromFuture {
            ahead_s: ahead_ms / 1000,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPIC: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const ID: &str = "00000000-0000-4000-8000-000000000000";
    const NOW: i64 = 1_700_000_000_000;

    fn key() -> Key {
        Key::from_b64("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8").unwrap()
    }

    fn body(json: &str) -> String {
        crypto::encrypt(&key(), TOPIC, json.as_bytes()).unwrap()
    }

    fn message(id: &str, code: &str, sender: &str, ts: i64) -> String {
        body(
            &serde_json::json!({ "v": 1, "id": id, "code": code, "sender": sender, "ts": ts })
                .to_string(),
        )
    }

    fn receive_at(body: &str, now_ms: i64) -> Result<OtpMessage, Reject> {
        receive(&key(), TOPIC, body, now_ms, &mut Dedupe::default())
    }

    fn reject_of(body: &str) -> Reject {
        match receive_at(body, NOW) {
            Err(reject) => reject,
            Ok(_) => panic!("message was unexpectedly accepted"),
        }
    }

    /// Plaintext JSON of exactly `len` bytes (padded through an unknown field).
    fn json_of_len(len: usize) -> String {
        let make = |pad: usize| {
            format!(
                r#"{{"v":1,"id":"{ID}","code":"123456","ts":{NOW},"pad":"{}"}}"#,
                "x".repeat(pad)
            )
        };
        let json = make(len - make(0).len());
        assert_eq!(json.len(), len);
        json
    }

    #[test]
    fn accepts_valid_message() {
        let msg = receive_at(&message(ID, "123456", "+41000000000", NOW), NOW).unwrap();
        assert_eq!(msg.v, 1);
        assert_eq!(msg.id, ID);
        assert_eq!(msg.code, "123456");
        assert_eq!(msg.sender, "+41000000000");
        assert_eq!(msg.ts, NOW);
    }

    #[test]
    fn encoded_plaintext_round_trips_through_receive() {
        let sender = r#"Bank "A" \ 24/7"#;
        let json = encode_plaintext(ID, "AbC123", sender, NOW);
        let msg = receive_at(&body(&json), NOW).unwrap();
        assert_eq!(msg.v, VERSION);
        assert_eq!(msg.id, ID);
        assert_eq!(msg.code, "AbC123");
        assert_eq!(msg.sender, sender);
        assert_eq!(msg.ts, NOW);
    }

    #[test]
    fn sender_is_optional_and_unknown_fields_are_ignored() {
        let json = format!(r#"{{"v":1,"id":"{ID}","code":"AbC123","ts":{NOW},"extra":[1,2]}}"#);
        let msg = receive_at(&body(&json), NOW).unwrap();
        assert_eq!(msg.sender, "");
        assert_eq!(msg.code, "AbC123");

        let json = format!(r#"{{"v":1,"id":"{ID}","code":"AbC123","sender":null,"ts":{NOW}}}"#);
        assert_eq!(receive_at(&body(&json), NOW).unwrap().sender, "");
    }

    #[test]
    fn undecryptable_bodies() {
        let valid = message(ID, "123456", "", NOW);
        // N1: other topic.
        let other = receive(
            &key(),
            "BBBBBBBBBBBBBBBBBBBBBB",
            &valid,
            NOW,
            &mut Dedupe::default(),
        );
        assert_eq!(other.err(), Some(Reject::Undecryptable));
        // N3: truncated.
        assert_eq!(reject_of(&valid[..20]), Reject::Undecryptable);
        assert_eq!(reject_of("not base64!"), Reject::Undecryptable);
    }

    #[test]
    fn oversize_body_is_too_large() {
        assert_eq!(
            reject_of(&"A".repeat(crypto::MAX_BODY_CHARS + 1)),
            Reject::TooLarge
        );
    }

    #[test]
    fn plaintext_size_boundary() {
        assert!(receive_at(&body(&json_of_len(MAX_PLAINTEXT_BYTES)), NOW).is_ok());
        assert_eq!(
            reject_of(&body(&json_of_len(MAX_PLAINTEXT_BYTES + 1))),
            Reject::TooLarge
        );
    }

    #[test]
    fn malformed_plaintext() {
        let cases = [
            "not json".to_owned(),
            "[]".to_owned(),
            r#"{"v":1}"#.to_owned(),
            format!(r#"{{"v":1,"id":"{ID}","code":123456,"ts":{NOW}}}"#),
            format!(r#"{{"v":1,"id":"{ID}","code":"123456","ts":"{NOW}"}}"#),
            format!(r#"{{"v":1,"id":"{ID}","code":"123456","ts":{NOW}.5}}"#),
            format!(r#"{{"v":300,"id":"{ID}","code":"123456","ts":{NOW}}}"#),
            format!(r#"{{"v":1,"id":"{ID}","code":"1","code":"123456","ts":{NOW}}}"#),
        ];
        for json in &cases {
            assert_eq!(reject_of(&body(json)), Reject::Malformed, "{json}");
        }
    }

    #[test]
    fn unsupported_version() {
        let json = format!(r#"{{"v":2,"id":"{ID}","code":"123456","ts":{NOW}}}"#);
        assert_eq!(reject_of(&body(&json)), Reject::UnsupportedVersion { v: 2 });
        let json = format!(r#"{{"v":0,"id":"{ID}","code":"123456","ts":{NOW}}}"#);
        assert_eq!(reject_of(&body(&json)), Reject::UnsupportedVersion { v: 0 });
    }

    #[test]
    fn id_rule_boundaries() {
        for ok in ["a".repeat(16), "Z".repeat(64), "abc_DEF-123_xyz-".into()] {
            assert!(
                receive_at(&message(&ok, "1234", "", NOW), NOW).is_ok(),
                "{ok}"
            );
        }
        for bad in [
            "a".repeat(15),
            "a".repeat(65),
            "aaaaaaaaaaaaaaa.".into(),
            String::new(),
        ] {
            assert_eq!(
                reject_of(&message(&bad, "1234", "", NOW)),
                Reject::InvalidId,
                "{bad}"
            );
        }
    }

    #[test]
    fn code_rule_boundaries() {
        for ok in ["1234", "1234567890", "AbCd12", "abcd"] {
            assert!(receive_at(&message(ID, ok, "", NOW), NOW).is_ok(), "{ok}");
        }
        // N5 is the 11-char case.
        for bad in [
            "123",
            "12345678901",
            "12 34",
            "12-34",
            "١٢٣٤",
            "１２３４",
            "",
        ] {
            assert_eq!(
                reject_of(&message(ID, bad, "", NOW)),
                Reject::InvalidCode,
                "{bad}"
            );
        }
    }

    #[test]
    fn sender_rule_boundaries() {
        for ok in ["x".repeat(64), "ä".repeat(64), "Bank 24/7".into()] {
            assert!(
                receive_at(&message(ID, "1234", &ok, NOW), NOW).is_ok(),
                "{ok}"
            );
        }
        for bad in [
            "x".repeat(65),
            "line\nbreak".into(),
            "tab\t".into(),
            "del\u{7f}".into(),
            "c1\u{85}".into(),
        ] {
            assert_eq!(
                reject_of(&message(ID, "1234", &bad, NOW)),
                Reject::InvalidSender,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn freshness_boundaries() {
        let at = |ts: i64| receive_at(&message(ID, "1234", "", ts), NOW);
        assert!(at(NOW - MAX_AGE_MS).is_ok());
        assert_eq!(
            at(NOW - MAX_AGE_MS - 1).err(),
            Some(Reject::Stale { age_s: 120 })
        );
        // N4: 121 s old.
        assert_eq!(at(NOW - 121_000).err(), Some(Reject::Stale { age_s: 121 }));
        assert!(at(NOW + MAX_FUTURE_MS).is_ok());
        assert_eq!(
            at(NOW + MAX_FUTURE_MS + 1).err(),
            Some(Reject::FromFuture { ahead_s: 30 })
        );
    }

    #[test]
    fn extreme_timestamps_do_not_overflow() {
        assert!(matches!(
            check_freshness(i64::MIN, i64::MAX),
            Err(Reject::Stale { .. })
        ));
        assert!(matches!(
            check_freshness(i64::MAX, i64::MIN),
            Err(Reject::FromFuture { .. })
        ));
    }

    #[test]
    fn duplicate_is_rejected_until_ttl_expires() {
        let mut seen = Dedupe::default();
        let first = message(ID, "123456", "", NOW);
        // N6: same message twice; also a re-encryption with the same id.
        assert!(receive(&key(), TOPIC, &first, NOW, &mut seen).is_ok());
        assert_eq!(
            receive(&key(), TOPIC, &first, NOW, &mut seen).err(),
            Some(Reject::Duplicate)
        );
        let resent = message(ID, "123456", "", NOW + 60_000);
        assert_eq!(
            receive(&key(), TOPIC, &resent, NOW + 60_000, &mut seen).err(),
            Some(Reject::Duplicate)
        );

        let later = NOW + DEDUPE_TTL_MS - 1;
        let msg = message(ID, "123456", "", later);
        assert_eq!(
            receive(&key(), TOPIC, &msg, later, &mut seen).err(),
            Some(Reject::Duplicate)
        );
        let expired = NOW + DEDUPE_TTL_MS;
        let msg = message(ID, "123456", "", expired);
        assert!(receive(&key(), TOPIC, &msg, expired, &mut seen).is_ok());
    }

    #[test]
    fn rejected_messages_are_not_recorded() {
        let mut seen = Dedupe::default();
        let stale = message(ID, "123456", "", NOW - MAX_AGE_MS - 1);
        assert!(receive(&key(), TOPIC, &stale, NOW, &mut seen).is_err());
        let fresh = message(ID, "123456", "", NOW);
        assert!(receive(&key(), TOPIC, &fresh, NOW, &mut seen).is_ok());
    }

    #[test]
    fn reject_serializes_with_kind_tag() {
        let json = serde_json::to_string(&Reject::Stale { age_s: 5 }).unwrap();
        assert_eq!(json, r#"{"kind":"stale","age_s":5}"#);
        let json = serde_json::to_string(&Reject::Duplicate).unwrap();
        assert_eq!(json, r#"{"kind":"duplicate"}"#);
    }
}
