# Wire contract v1 (desktop ↔ Android)

Status: **frozen**. Any change needs architect sign-off, a version bump (see [Versioning](#10-versioning)) and a
mirrored change in the Android repo. The desktop implementation lives in `src-tauri/src/{crypto,otp,pairing}.rs`;
its unit tests pin the rules and test vectors below.

## 1. Overview and threat model

```
phone (SMS) ──POST ciphertext──▶ ntfy relay ──GET stream──▶ desktop ──types code──▶ focused app
```

- The **relay is untrusted**. It sees only the topic name, ciphertext, sizes and timing. Confidentiality and
  authenticity come from AES-256-GCM with a key shared once via the pairing QR code.
- The **topic** is a random 128-bit name. It is secret-ish (it keeps strangers from spamming the stream) but it is
  **not** a security boundary. The AAD binds every message to its topic.
- **Replay and delay** are bounded by the freshness window (120 s old / 30 s ahead) and by per-id dedupe (10 min).
  The dedupe set lives in memory, so after a desktop restart a hostile relay could replay a message that is still
  inside the freshness window. The result is that the same, already-used code is typed again. This is accepted.
- **Anyone who can send an SMS to the phone** can make the desktop type 4–10 alphanumeric characters into an
  allowlisted app. Mitigations: the Android app MUST only forward SMS from senders the user allowed, and
  `press_enter` is off by default, so a typed code is never submitted without the user.
- The key never leaves the two devices. On the desktop it is stored in the OS keyring (Windows Credential Manager).
  Codes are never logged or written to disk.
- Out of scope: a compromised phone or desktop, and an attacker who can read the pairing QR code. Re-pairing
  creates a new topic and key.

## 2. Encoding conventions

- **base64url** = RFC 4648 §5 alphabet (`A–Z a–z 0–9 - _`). Encoders MUST NOT pad. Decoders MUST accept both padded
  and unpadded input. Characters outside the alphabet (including `+` and `/`) are errors.
- All JSON is UTF-8. Receivers ignore unknown fields.

## 3. Pairing payload (QR code)

Shown by the desktop as a QR code (error correction level M) and scanned by the phone. The same JSON is the keyring
secret on the desktop and the input of the `otpfwd-send` CLI.

```json
{"v":1,"relay":"https://ntfy.sh","topic":"<22 chars>","key":"<43 chars>"}
```

| Field   | Rule                                                                                                 |
|---------|------------------------------------------------------------------------------------------------------|
| `v`     | `1`; anything else is rejected                                                                       |
| `relay` | absolute `https` URL, origin only: no userinfo, no path other than `/`, no query, no fragment. A trailing `/` is stripped when writing |
| `topic` | `^[A-Za-z0-9_-]{16,64}$`. The desktop generates base64url of 16 random bytes (22 chars)             |
| `key`   | base64url of exactly 32 random bytes (43 chars unpadded)                                             |

## 4. AEAD

- AES-256-GCM, 96-bit nonce, 128-bit tag (Java/Kotlin: `AES/GCM/NoPadding`, `GCMParameterSpec(128, nonce)`).
- **AAD** = UTF-8 bytes of `"otpfwd/v1/" + topic`, for example `otpfwd/v1/AAAAAAAAAAAAAAAAAAAAAA`.
- **Nonce**: 12 bytes from a CSPRNG, fresh for every encryption. Never derive it from a counter or the message.
- **Body** = `base64url(nonce[12] ‖ ciphertext ‖ tag[16])`, ASCII, at most **1024 chars**. It must decode to at least
  29 bytes (at least one plaintext byte).

Kotlin sketch:

```kotlin
val cipher = Cipher.getInstance("AES/GCM/NoPadding")
cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
cipher.updateAAD("otpfwd/v1/$topic".toByteArray(Charsets.UTF_8))
val body = Base64.getUrlEncoder().withoutPadding().encodeToString(nonce + cipher.doFinal(plaintext))
```

## 5. Plaintext

UTF-8 JSON, at most **512 bytes**. Unknown fields are ignored. Duplicate fields are rejected.

```json
{"v":1,"id":"<16–64 [A-Za-z0-9_-]>","code":"123456","sender":"+41…","ts":1700000000000}
```

| Field    | Rule                                                                                    |
|----------|-----------------------------------------------------------------------------------------|
| `v`      | integer, must be `1`                                                                    |
| `id`     | `^[A-Za-z0-9_-]{16,64}$`. A UUIDv4 is recommended on Android; the desktop CLI uses base64url of 16 random bytes |
| `code`   | `^[A-Za-z0-9]{4,10}$` (ASCII only), typed verbatim with case preserved                  |
| `sender` | optional, default `""`, at most 64 characters, no control characters                    |
| `ts`     | integer Unix time in **milliseconds** (phone clock), when the message was created       |

## 6. Publish (phone → relay)

```
POST {relay}/{topic}
X-Firebase: no

<body>
```

- `X-Firebase: no` keeps the ciphertext off Firebase Cloud Messaging. Send no title, tags or other ntfy headers.
- On 429 or 5xx, retry with backoff and re-send the **identical body**. The id stays the same and dedupe makes the
  retry idempotent. Do not re-encrypt a retry.

## 7. Subscribe (desktop ← relay)

```
GET {relay}/{topic}/json?since=<unix seconds>
```

- NDJSON stream, one event per line. Only `open`, `keepalive` and `message` events are used; others are ignored.
  The `message` field of a `message` event is the body. A `message` event without a `message` field (an attachment,
  over 4096 bytes) is ignored. Lines longer than 16 KiB drop the connection.
- Keepalives arrive about every 45 s. Every event carries the server time `time` (Unix seconds).
- **`since`**: on the first connect after process start, `since = process start (unix s) − 5`. On reconnect,
  `since = max(time of any event seen) − 5`. This uses the server's clock, so it is skew-free, and the overlap is
  absorbed by dedupe. `since` is not a security control; freshness and dedupe are.
- ntfy.sh caches messages for about 12 h; `since` catch-up relies on that. Catch-up after sleep can replay old
  messages; they are rejected as `stale`.

## 8. Desktop acceptance

Checked in this order; the first failure rejects the message with the reason in parentheses:

1. Body at most 1024 chars (`too_large`).
2. base64url decodes to at least 29 bytes and the GCM tag verifies (`undecryptable`).
3. Plaintext at most 512 bytes (`too_large`) and valid JSON of the shape above (`malformed`).
4. `v == 1` (`unsupported_version`).
5. `id` (`invalid_id`), `code` (`invalid_code`) and `sender` (`invalid_sender`) pass the field rules.
6. `now − ts ≤ 120 s` (`stale`) and `ts − now ≤ 30 s` (`from_future`). Both bounds are inclusive.
7. `id` not accepted within the last 10 min (`duplicate`). Only fully valid messages are recorded.

## 9. Test vectors

Common inputs:

- key = bytes `00 01 … 1f` → base64url `AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8`
- nonce = bytes `00 01 … 0b`
- topic = `AAAAAAAAAAAAAAAAAAAAAA` → AAD `otpfwd/v1/AAAAAAAAAAAAAAAAAAAAAA`
- plaintext (exact 110 bytes, no whitespace):
  `{"v":1,"id":"00000000-0000-4000-8000-000000000000","code":"123456","sender":"+41000000000","ts":1700000000000}`

| #  | Case                                    | Expected                     |
|----|-----------------------------------------|------------------------------|
| V1 | encrypt with the inputs above           | body below (184 chars)       |
| N1 | V1 body, topic `BBBBBBBBBBBBBBBBBBBBBB` | `undecryptable`              |
| N2 | V1 body with any single bit flipped     | `undecryptable`              |
| N3 | first 20 chars of the V1 body           | `undecryptable`              |
| N4 | V1 at now = ts + 121 s                  | `stale` (`age_s` = 121)      |
| N5 | code `12345678901`                      | `invalid_code`               |
| N6 | V1 received twice                       | second one is `duplicate`    |

V1 body (cross-checked with Node `crypto`):

```
AAECAwQFBgcICQoLPCCgOf_U7jnkJbWxk9lIXbPmtwTAVm9MCFfIsS1ZMJ85IJ7MgvEiqESUT924txgI3ntMrzm5x78drQgoKtDB28Ye6ligtEgFeSaIVM2kOtkIuvlSRlNjWM3fsu4Mg5u3-s9p1pRx-CquLA6CeAhsRUgSYr9_WFD7G9Hj2068
```

## 10. Versioning

Any change to this contract means `v: 2` in both the pairing payload and the plaintext, AAD prefix `otpfwd/v2/`, and
re-pairing. Receivers reject unknown versions.
