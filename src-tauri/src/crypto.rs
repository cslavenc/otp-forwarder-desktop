//! AES-256-GCM sealing of relay message bodies (wire contract v1, see docs/wire-contract.md).

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD_INDIFFERENT;
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
/// Random bytes behind a topic name (22 base64url chars).
pub const TOPIC_BYTES: usize = 16;
/// Max length of a message body (base64url text) in characters.
pub const MAX_BODY_CHARS: usize = 1024;
const AAD_PREFIX: &str = "otpfwd/v1/";
const TOKEN_MIN_LEN: usize = 16;
const TOKEN_MAX_LEN: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("random number generator failed: {0}")]
    Rng(#[from] getrandom::Error),
    #[error("message body too large")]
    TooLarge,
    #[error("malformed base64url or wrong length")]
    Malformed,
    #[error("authentication failed")]
    Auth,
}

/// AES-256 key, zeroized on drop. Deliberately neither `Debug` nor `Display`.
#[derive(Clone)]
pub struct Key(Zeroizing<[u8; KEY_LEN]>);

impl Key {
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(bytes.as_mut_slice())?;
        Ok(Self(bytes))
    }

    /// Accepts padded and unpadded base64url; must decode to exactly 32 bytes.
    pub fn from_b64(s: &str) -> Result<Self, CryptoError> {
        let decoded = Zeroizing::new(b64_decode(s)?);
        if decoded.len() != KEY_LEN {
            return Err(CryptoError::Malformed);
        }
        let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
        bytes.copy_from_slice(&decoded);
        Ok(Self(bytes))
    }

    pub fn to_b64(&self) -> Zeroizing<String> {
        Zeroizing::new(b64(self.0.as_slice()))
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new((&*self.0).into())
    }
}

/// New random topic: base64url of 16 random bytes (22 chars).
pub fn generate_topic() -> Result<String, CryptoError> {
    Ok(b64(&random::<TOPIC_BYTES>()?))
}

/// Topic and message id rule: 16–64 chars of `[A-Za-z0-9_-]`.
pub(crate) fn is_valid_token(s: &str) -> bool {
    (TOKEN_MIN_LEN..=TOKEN_MAX_LEN).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Seals `plaintext` with a fresh random nonce; returns `base64url(nonce ‖ ciphertext ‖ tag)`.
pub fn encrypt(key: &Key, topic: &str, plaintext: &[u8]) -> Result<String, CryptoError> {
    seal(key, topic, &random::<NONCE_LEN>()?, plaintext)
}

/// Opens a message body produced by [`encrypt`] for the same key and topic.
pub fn decrypt(key: &Key, topic: &str, body: &str) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if body.len() > MAX_BODY_CHARS {
        return Err(CryptoError::TooLarge);
    }
    let raw = b64_decode(body)?;
    let (nonce, sealed) = raw
        .split_first_chunk::<NONCE_LEN>()
        .ok_or(CryptoError::Malformed)?;
    // Contract: at least one plaintext byte besides nonce and tag.
    if sealed.len() <= TAG_LEN {
        return Err(CryptoError::Malformed);
    }
    let aad = aad(topic);
    key.cipher()
        .decrypt(
            nonce.into(),
            Payload {
                msg: sealed,
                aad: aad.as_bytes(),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Auth)
}

/// Explicit-nonce variant, private so callers cannot reuse nonces; tests use it for vectors.
fn seal(
    key: &Key,
    topic: &str,
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> Result<String, CryptoError> {
    let aad = aad(topic);
    // aes-gcm only fails for plaintexts far beyond any body we accept.
    let sealed = key
        .cipher()
        .encrypt(
            nonce.into(),
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| CryptoError::TooLarge)?;
    let mut raw = Vec::with_capacity(NONCE_LEN + sealed.len());
    raw.extend_from_slice(nonce);
    raw.extend_from_slice(&sealed);
    let body = b64(&raw);
    if body.len() > MAX_BODY_CHARS {
        return Err(CryptoError::TooLarge);
    }
    Ok(body)
}

fn aad(topic: &str) -> String {
    format!("{AAD_PREFIX}{topic}")
}

fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes)?;
    Ok(bytes)
}

/// Unpadded base64url (encoders must not pad).
fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD_INDIFFERENT.encode(bytes)
}

/// base64url, padding optional.
fn b64_decode(s: &str) -> Result<Vec<u8>, CryptoError> {
    URL_SAFE_NO_PAD_INDIFFERENT
        .decode(s)
        .map_err(|_| CryptoError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPIC: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const OTHER_TOPIC: &str = "BBBBBBBBBBBBBBBBBBBBBB";
    const PLAINTEXT: &str = r#"{"v":1,"id":"00000000-0000-4000-8000-000000000000","code":"123456","sender":"+41000000000","ts":1700000000000}"#;
    /// Wire contract test vector V1, computed independently with Node `crypto`
    /// (aes-256-gcm, setAAD, base64url). Must match docs/wire-contract.md.
    const V1_BODY: &str = "AAECAwQFBgcICQoLPCCgOf_U7jnkJbWxk9lIXbPmtwTAVm9MCFfIsS1ZMJ85IJ7MgvEiqESUT924txgI3ntMrzm5x78drQgoKtDB28Ye6ligtEgFeSaIVM2kOtkIuvlSRlNjWM3fsu4Mg5u3-s9p1pRx-CquLA6CeAhsRUgSYr9_WFD7G9Hj2068";
    /// base64url of bytes 00..1f.
    const KEY_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

    fn vector_key() -> Key {
        Key(Zeroizing::new(std::array::from_fn(|i| i as u8)))
    }

    fn vector_nonce() -> [u8; NONCE_LEN] {
        std::array::from_fn(|i| i as u8)
    }

    fn assert_err(result: Result<Zeroizing<Vec<u8>>, CryptoError>, expected: &str) {
        match result {
            Err(e) => assert_eq!(format!("{e:?}"), expected),
            Ok(_) => panic!("expected {expected}, got plaintext"),
        }
    }

    #[test]
    fn known_answer_vector_v1() {
        let body = seal(&vector_key(), TOPIC, &vector_nonce(), PLAINTEXT.as_bytes()).unwrap();
        assert_eq!(body, V1_BODY);
        let opened = decrypt(&vector_key(), TOPIC, V1_BODY).unwrap();
        assert_eq!(opened.as_slice(), PLAINTEXT.as_bytes());
    }

    #[test]
    fn vector_key_matches_its_base64url() {
        assert_eq!(vector_key().to_b64().as_str(), KEY_B64);
        let parsed = Key::from_b64(KEY_B64).unwrap();
        assert_eq!(parsed.0.as_slice(), vector_key().0.as_slice());
    }

    #[test]
    fn encrypt_uses_fresh_nonce_and_round_trips() {
        let key = Key::generate().unwrap();
        let a = encrypt(&key, TOPIC, b"hello").unwrap();
        let b = encrypt(&key, TOPIC, b"hello").unwrap();
        assert_ne!(a, b);
        assert_eq!(decrypt(&key, TOPIC, &a).unwrap().as_slice(), b"hello");
        assert_eq!(decrypt(&key, TOPIC, &b).unwrap().as_slice(), b"hello");
    }

    #[test]
    fn any_flipped_bit_fails_authentication() {
        let key = vector_key();
        let raw = b64_decode(&encrypt(&key, TOPIC, PLAINTEXT.as_bytes()).unwrap()).unwrap();
        for bit in 0..raw.len() * 8 {
            let mut tampered = raw.clone();
            tampered[bit / 8] ^= 1 << (bit % 8);
            assert_err(decrypt(&key, TOPIC, &b64(&tampered)), "Auth");
        }
    }

    #[test]
    fn wrong_topic_fails_authentication() {
        let body = encrypt(&vector_key(), TOPIC, PLAINTEXT.as_bytes()).unwrap();
        assert_err(decrypt(&vector_key(), OTHER_TOPIC, &body), "Auth");
    }

    #[test]
    fn wrong_key_fails_authentication() {
        let body = encrypt(&vector_key(), TOPIC, PLAINTEXT.as_bytes()).unwrap();
        assert_err(decrypt(&Key::generate().unwrap(), TOPIC, &body), "Auth");
    }

    #[test]
    fn body_length_cap_is_enforced_before_decoding() {
        let key = vector_key();
        assert_err(
            decrypt(&key, TOPIC, &"A".repeat(MAX_BODY_CHARS + 1)),
            "TooLarge",
        );
        // Exactly at the cap passes the length check and fails authentication instead.
        assert_err(decrypt(&key, TOPIC, &"A".repeat(MAX_BODY_CHARS)), "Auth");
    }

    #[test]
    fn encrypt_refuses_bodies_over_the_cap() {
        let key = vector_key();
        // 740 B plaintext + 28 B overhead = 768 B = 1024 base64 chars.
        let fits = vec![b'x'; 740];
        assert_eq!(encrypt(&key, TOPIC, &fits).unwrap().len(), MAX_BODY_CHARS);
        let too_big = vec![b'x'; 741];
        assert!(matches!(
            encrypt(&key, TOPIC, &too_big),
            Err(CryptoError::TooLarge)
        ));
    }

    #[test]
    fn malformed_bodies_are_rejected() {
        let key = vector_key();
        assert_err(decrypt(&key, TOPIC, "!!!not base64!!!"), "Malformed");
        assert_err(decrypt(&key, TOPIC, ""), "Malformed");
        // Standard-alphabet characters are not base64url.
        assert_err(decrypt(&key, TOPIC, "ab+/"), "Malformed");
        // nonce + tag only (28 bytes), no plaintext byte.
        assert_err(decrypt(&key, TOPIC, &b64(&[0u8; 28])), "Malformed");
        // N3: truncated body.
        let body = encrypt(&key, TOPIC, PLAINTEXT.as_bytes()).unwrap();
        assert_err(decrypt(&key, TOPIC, &body[..20]), "Malformed");
    }

    #[test]
    fn key_from_b64_accepts_padding_and_checks_length() {
        assert!(Key::from_b64(&format!("{KEY_B64}=")).is_ok());
        assert!(matches!(
            Key::from_b64(&b64(&[0u8; 31])),
            Err(CryptoError::Malformed)
        ));
        assert!(matches!(
            Key::from_b64(&b64(&[0u8; 33])),
            Err(CryptoError::Malformed)
        ));
        assert!(matches!(Key::from_b64("%%%"), Err(CryptoError::Malformed)));
    }

    #[test]
    fn generated_keys_and_topics_are_random_and_well_formed() {
        let key = Key::generate().unwrap();
        assert_eq!(key.to_b64().len(), 43);
        assert_ne!(
            key.to_b64().as_str(),
            Key::generate().unwrap().to_b64().as_str()
        );

        let topic = generate_topic().unwrap();
        assert_eq!(topic.len(), 22);
        assert!(is_valid_token(&topic));
        assert_ne!(topic, generate_topic().unwrap());
    }

    #[test]
    fn token_rule_boundaries() {
        assert!(!is_valid_token(&"a".repeat(15)));
        assert!(is_valid_token(&"a".repeat(16)));
        assert!(is_valid_token(&"a".repeat(64)));
        assert!(!is_valid_token(&"a".repeat(65)));
        assert!(is_valid_token("Az09_-Az09_-Az09"));
        assert!(!is_valid_token("aaaaaaaaaaaaaaa="));
        assert!(!is_valid_token("aaaaaaaaaaaaaaa/"));
        assert!(!is_valid_token("aaaaaaaaaaaaaaaä"));
    }
}
