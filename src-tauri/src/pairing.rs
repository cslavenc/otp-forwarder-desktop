//! Pairing data (relay, topic, key): QR payload, validation, and keyring persistence.

use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::Zeroizing;

use crate::crypto::{self, CryptoError, Key};

pub const VERSION: u8 = 1;
pub const DEFAULT_RELAY: &str = "https://ntfy.sh";
/// Keyring location of the pairing JSON; shared by the app and the sender CLI.
pub const KEYRING_SERVICE: &str = "ch.cslavenc.otpforwarder";
pub const KEYRING_USER: &str = "pairing";
const QR_MIN_PX: u32 = 256;

/// No `Debug`/`Serialize`: it holds the key. Use [`Pairing::to_json`] deliberately.
#[derive(Clone)]
pub struct Pairing {
    /// Origin only (`https`, no credentials, path, query or fragment).
    pub relay: Url,
    pub topic: String,
    pub key: Key,
}

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("invalid pairing: {0}")]
    Invalid(&'static str),
    /// keyring's `Display` text only; its `Debug` could include secret bytes.
    #[error("keyring: {0}")]
    Keyring(String),
    #[error("QR code: {0}")]
    Qr(String),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
}

#[derive(Deserialize)]
struct WireIn {
    v: u8,
    relay: String,
    topic: String,
    key: String,
}

#[derive(Serialize)]
struct WireOut<'a> {
    v: u8,
    relay: &'a str,
    topic: &'a str,
    key: &'a str,
}

impl Pairing {
    pub fn generate(relay: &str) -> Result<Self, PairingError> {
        Ok(Self {
            relay: parse_relay(relay)?,
            topic: crypto::generate_topic()?,
            key: Key::generate()?,
        })
    }

    /// Parses and fully validates pairing JSON (unknown fields are ignored).
    pub fn from_json(json: &str) -> Result<Self, PairingError> {
        // No serde error text: it can quote input, which may contain the key.
        let WireIn {
            v,
            relay,
            topic,
            key,
        } = serde_json::from_str(json).map_err(|_| PairingError::Invalid("malformed JSON"))?;
        let key = Zeroizing::new(key);
        if v != VERSION {
            return Err(PairingError::Invalid("unsupported version"));
        }
        if !crypto::is_valid_token(&topic) {
            return Err(PairingError::Invalid(
                "topic must be 16-64 base64url characters",
            ));
        }
        let key = Key::from_b64(&key)
            .map_err(|_| PairingError::Invalid("key must be 32 bytes of base64url"))?;
        Ok(Self {
            relay: parse_relay(&relay)?,
            topic,
            key,
        })
    }

    /// Contract JSON; contains the key.
    pub fn to_json(&self) -> Result<Zeroizing<String>, PairingError> {
        let key = self.key.to_b64();
        let wire = WireOut {
            v: VERSION,
            relay: self.relay.as_str().trim_end_matches('/'),
            topic: &self.topic,
            key: &key,
        };
        serde_json::to_string(&wire)
            .map(Zeroizing::new)
            .map_err(|_| PairingError::Invalid("could not serialize pairing"))
    }

    /// SVG QR code of [`Pairing::to_json`] (ECC M); contains the key.
    pub fn qr_svg(&self) -> Result<String, PairingError> {
        let json = self.to_json()?;
        let code = QrCode::with_error_correction_level(json.as_bytes(), EcLevel::M)
            .map_err(|e| PairingError::Qr(e.to_string()))?;
        Ok(code
            .render::<svg::Color>()
            .min_dimensions(QR_MIN_PX, QR_MIN_PX)
            .build())
    }

    /// `Ok(None)` only when no pairing is stored; any other failure is an error so callers
    /// never overwrite an unreadable pairing.
    pub fn load() -> Result<Option<Self>, PairingError> {
        Self::load_from(&keyring_entry(KEYRING_SERVICE)?)
    }

    pub fn save(&self) -> Result<(), PairingError> {
        self.save_to(&keyring_entry(KEYRING_SERVICE)?)
    }

    fn load_from(entry: &keyring::Entry) -> Result<Option<Self>, PairingError> {
        match entry.get_password() {
            Ok(json) => Self::from_json(&Zeroizing::new(json)).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error(e)),
        }
    }

    fn save_to(&self, entry: &keyring::Entry) -> Result<(), PairingError> {
        entry.set_password(&self.to_json()?).map_err(keyring_error)
    }
}

fn keyring_entry(service: &str) -> Result<keyring::Entry, PairingError> {
    keyring::Entry::new(service, KEYRING_USER).map_err(keyring_error)
}

fn keyring_error(e: keyring::Error) -> PairingError {
    PairingError::Keyring(e.to_string())
}

fn parse_relay(relay: &str) -> Result<Url, PairingError> {
    let url =
        Url::parse(relay).map_err(|_| PairingError::Invalid("relay must be an absolute URL"))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err(PairingError::Invalid("relay must be an https URL"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(PairingError::Invalid("relay must not contain credentials"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(PairingError::Invalid(
            "relay must be an origin without path, query or fragment",
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPIC: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const KEY_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

    fn json(relay: &str, topic: &str, key: &str) -> String {
        format!(r#"{{"v":1,"relay":"{relay}","topic":"{topic}","key":"{key}"}}"#)
    }

    fn invalid_reason(json: &str) -> &'static str {
        match Pairing::from_json(json) {
            Err(PairingError::Invalid(reason)) => reason,
            Err(e) => panic!("unexpected error {e}"),
            Ok(_) => panic!("pairing unexpectedly valid: {json}"),
        }
    }

    #[test]
    fn contract_json_round_trips_exactly() {
        let input = json(DEFAULT_RELAY, TOPIC, KEY_B64);
        let pairing = Pairing::from_json(&input).unwrap();
        assert_eq!(pairing.topic, TOPIC);
        assert_eq!(pairing.key.to_b64().as_str(), KEY_B64);
        assert_eq!(pairing.to_json().unwrap().as_str(), input);
    }

    #[test]
    fn generated_pairing_round_trips() {
        let pairing = Pairing::generate(DEFAULT_RELAY).unwrap();
        assert_eq!(pairing.topic.len(), 22);
        let restored = Pairing::from_json(&pairing.to_json().unwrap()).unwrap();
        assert_eq!(restored.relay, pairing.relay);
        assert_eq!(restored.topic, pairing.topic);
        assert_eq!(
            restored.key.to_b64().as_str(),
            pairing.key.to_b64().as_str()
        );
    }

    #[test]
    fn relay_is_normalized() {
        for (input, expected) in [
            ("https://ntfy.sh/", "https://ntfy.sh"),
            ("HTTPS://NTFY.SH", "https://ntfy.sh"),
            ("https://relay.example:8443", "https://relay.example:8443"),
        ] {
            let pairing = Pairing::from_json(&json(input, TOPIC, KEY_B64)).unwrap();
            assert_eq!(
                pairing.to_json().unwrap().as_str(),
                json(expected, TOPIC, KEY_B64)
            );
        }
    }

    #[test]
    fn accepts_padded_key_and_ignores_unknown_fields() {
        assert!(Pairing::from_json(&json(DEFAULT_RELAY, TOPIC, &format!("{KEY_B64}="))).is_ok());
        let extra = format!(
            r#"{{"v":1,"relay":"{DEFAULT_RELAY}","topic":"{TOPIC}","key":"{KEY_B64}","note":"x"}}"#
        );
        assert!(Pairing::from_json(&extra).is_ok());
    }

    #[test]
    fn rejects_bad_relays() {
        for relay in [
            "http://ntfy.sh",
            "ftp://ntfy.sh",
            "ntfy.sh",
            "https://user:pw@ntfy.sh",
            "https://user@ntfy.sh",
            "https://ntfy.sh/topic",
            "https://ntfy.sh//",
            "https://ntfy.sh/?a=b",
            "https://ntfy.sh/?",
            "https://ntfy.sh/#frag",
            "",
        ] {
            invalid_reason(&json(relay, TOPIC, KEY_B64));
            assert!(Pairing::generate(relay).is_err(), "{relay}");
        }
    }

    #[test]
    fn rejects_bad_topics_and_keys() {
        for topic in [
            "A".repeat(15),
            "A".repeat(65),
            "AAAAAAAAAAAAAAAAAAAAA=".into(),
        ] {
            assert_eq!(
                invalid_reason(&json(DEFAULT_RELAY, &topic, KEY_B64)),
                "topic must be 16-64 base64url characters"
            );
        }
        let keys = [
            KEY_B64[..42].to_owned(),
            String::new(),
            "AAEC+wQF".to_owned(),
            format!("{KEY_B64}AAAA"),
        ];
        for key in &keys {
            assert_eq!(
                invalid_reason(&json(DEFAULT_RELAY, TOPIC, key)),
                "key must be 32 bytes of base64url"
            );
        }
    }

    #[test]
    fn rejects_bad_shape_and_version() {
        assert_eq!(invalid_reason("not json"), "malformed JSON");
        assert_eq!(
            invalid_reason(&format!(
                r#"{{"v":1,"relay":"{DEFAULT_RELAY}","topic":"{TOPIC}"}}"#
            )),
            "malformed JSON"
        );
        assert_eq!(
            invalid_reason(&format!(
                r#"{{"v":2,"relay":"{DEFAULT_RELAY}","topic":"{TOPIC}","key":"{KEY_B64}"}}"#
            )),
            "unsupported version"
        );
    }

    #[test]
    fn qr_svg_is_generated_for_longest_valid_pairing() {
        let topic = "A".repeat(64);
        let relay = format!("https://{}.example", "r".repeat(60));
        let pairing = Pairing::from_json(&json(&relay, &topic, KEY_B64)).unwrap();
        let svg = pairing.qr_svg().unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }

    /// Touches the real OS keyring under a separate service name.
    /// Run with `cargo test -- --ignored keyring`.
    #[test]
    #[ignore = "uses the OS keyring"]
    fn keyring_round_trip() {
        let entry = keyring_entry("ch.cslavenc.otpforwarder.test").unwrap();
        let _ = entry.delete_credential();
        assert!(Pairing::load_from(&entry).unwrap().is_none());

        let pairing = Pairing::generate(DEFAULT_RELAY).unwrap();
        pairing.save_to(&entry).unwrap();
        let loaded = Pairing::load_from(&entry).unwrap().unwrap();
        assert_eq!(loaded.topic, pairing.topic);

        entry.set_password("{corrupt").unwrap();
        assert!(Pairing::load_from(&entry).is_err());
        entry.delete_credential().unwrap();
    }
}
