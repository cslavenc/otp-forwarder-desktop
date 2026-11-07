# OTP Forwarder Desktop

Forward SMS one-time passwords from your Android phone to your Windows PC and have them **typed automatically** into
the app that asked for them — Citrix Workspace, browser logins, VPN clients, and so on.

> **Status:** planning. The tech stack and architecture are decided; the project is not scaffolded yet.

## How it works

```
Android phone (SMS) ──AES-GCM encrypted──▶ ntfy.sh relay ──▶ OTP Forwarder Desktop ──▶ focused app (Citrix, …)
```

1. **Pair once:** the desktop app shows a QR code containing a random relay topic and a 256-bit key. The Android
   companion app scans it.
2. **Receive:** when an OTP SMS arrives, the phone extracts the code, encrypts it, and publishes it to the relay.
3. **Deliver:** the desktop app (running in the tray) receives and decrypts it, then:
   - **auto-types** the code into the focused window if that app is on your allowlist (optionally pressing Enter), or
   - copies it to the clipboard and shows a notification.

Works whether the phone is on the same Wi-Fi or on mobile data — the desktop only makes outbound connections, so no
ports or firewall rules are needed.

## Features (planned)

- QR-code pairing, end-to-end encryption (the relay only sees ciphertext)
- Auto-type into allowlisted apps via Windows `SendInput`, with clipboard + toast fallback
- Per-app rules: press Enter after typing, typing delay
- Short-lived, in-memory history; clipboard auto-clear
- System tray, start with Windows
- Windows 10 and Windows 11

## Tech stack

| Part             | Technology                                              |
|------------------|---------------------------------------------------------|
| Desktop app      | [Tauri v2](https://v2.tauri.app/) (Rust)                |
| UI               | Svelte 5 + TypeScript + Vite, Pico.css                  |
| Relay            | [ntfy.sh](https://ntfy.sh) (public, swappable for self-hosted) |
| Encryption       | AES-256-GCM                                             |
| Key storage      | Windows Credential Manager (via `keyring`)              |
| Android app      | Kotlin, SMS only — separate repository                  |

## Security model

- Codes are encrypted on the phone and decrypted only on the paired PC; ntfy.sh never sees plaintext or the key.
- Messages are rejected if they fail authentication, are stale, or are replays.
- OTPs are never logged or written to disk.
- Auto-typing only happens into allowlisted applications, so a code is never typed into a random chat window.
- Limitation: Windows does not allow typing into elevated (administrator) windows from a normal app.

## Development

Prerequisites:

- Windows 10 or 11
- [Rust](https://rustup.rs/) stable (MSVC toolchain)
- Visual Studio Build Tools with **Desktop development with C++**
- Node.js LTS
- WebView2 runtime (preinstalled on Windows 11 and current Windows 10)

Commands (available once scaffolded):

```powershell
npm install
npm run tauri dev       # run in development
npm run tauri build     # build the installer
npm run check           # frontend type-check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

## Roadmap

1. Scaffold Tauri v2 + Svelte 5 project
2. ntfy relay subscriber + AES-GCM decryption
3. QR pairing + key storage
4. Foreground-app detection, allowlist rules, auto-type, clipboard/toast fallback
5. Tray, autostart, installer
6. Android companion app (separate repo)
7. Later: Linux (Ubuntu) support

## Contributing with AI agents

See [`CLAUDE.md`](CLAUDE.md) for project conventions and the subagent workflow; agent definitions live in
[`.claude/agents/`](.claude/agents/).
