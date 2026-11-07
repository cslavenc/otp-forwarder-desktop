# OTP Forwarder Desktop

Forward SMS one-time passwords from your Android phone to your Windows PC and have them **typed automatically** into
the app that asked for them — Citrix Workspace, browser logins, VPN clients, and so on.

> **Status:** desktop app complete (v0.1). The Android companion app lives in a separate repository; until it exists
> you can test everything with the built-in test button or the `otpfwd-send` CLI (see [Testing](#testing)).

## How it works

```
Android phone (SMS) ──AES-GCM encrypted──▶ ntfy.sh relay ──▶ OTP Forwarder Desktop ──▶ focused app (Citrix, …)
```

1. **Pair once:** the desktop app shows a QR code containing a random relay topic and a 256-bit key. The Android
   companion app scans it.
2. **Receive:** when an OTP SMS arrives, the phone extracts the code, encrypts it, and publishes it to the relay.
3. **Deliver:** the desktop app (running in the tray) receives and decrypts it, then:
   - **auto-types** the code into the focused window if that app is on your allowlist (optionally pressing Enter), or
   - copies it to the clipboard (auto-cleared after 30 s) and shows a notification.

Works whether the phone is on the same Wi-Fi or on mobile data — the desktop only makes outbound connections, so no
ports or firewall rules are needed. The exact wire format is in [`docs/wire-contract.md`](docs/wire-contract.md).

## Features

- QR-code pairing, end-to-end AES-256-GCM encryption (the relay only sees ciphertext); key in Windows Credential Manager
- Rejects messages that fail authentication, are older than 2 minutes, come from the future, or are replays
- Auto-type into allowlisted apps (default: `wfica32.exe`, `CDViewer.exe`, `SelfService.exe`) via `SendInput`,
  keyboard-layout aware; clipboard + toast fallback everywhere else
- Per-app rules: press Enter after typing, delay between keystrokes
- Global hotkey **Ctrl+Alt+Shift+O** types the latest code into the focused app
- Recent codes (masked, in memory only, gone after 5 minutes); clipboard auto-clear, also on exit
- Tray icon with relay status; closing the window hides it; optional start with Windows
- Windows 10 and Windows 11

## Install and first run

1. Build the installer (see [Development](#development)) and run
   `src-tauri\target\release\bundle\nsis\OTP Forwarder_0.1.0_x64-setup.exe`.
2. Start **OTP Forwarder**. On first start it creates a pairing and opens the **Pairing** tab.
3. Click **Show QR code** and scan it with the Android app. The badge in the top-right should say **Connected**.
4. Optional: in **Settings**, enable *Start with Windows*, adjust the allowlist, and click **Save**.

Settings are stored in `%APPDATA%\ch.cslavenc.otpforwarder\settings.json`; the pairing key in Windows Credential
Manager (generic credential `ch.cslavenc.otpforwarder`). **Re-pair** creates a new key and topic — the phone must scan
again.

## Testing

You can test the whole path (relay → decryption → typing) without the Android app.

**From the app (no tools needed):** open **Codes → Send test code**, then within 5 seconds click into the target field
(e.g. a Citrix login prompt, or Notepad if you temporarily add `notepad.exe` to the allowlist). The app publishes an
encrypted random code to its own relay topic and delivers it like a real one.

**From the command line** (needs the Rust toolchain; reads the pairing from Credential Manager):

```powershell
cargo build --manifest-path src-tauri/Cargo.toml --example otpfwd-send
$send = "src-tauri\target\debug\examples\otpfwd-send.exe"
& $send --code 123456                              # normal code; prints the message id
& $send --code 654321 --delay-secs 5               # 5 s to focus the target field first
& $send --code 111111 --age-secs 600               # stale: must be rejected ("…was 600 s old…")
& $send --code 222222 --id ReplayTest0000001       # then run the same line again: 2nd must be rejected as duplicate
& $send --code 333333 --pairing other-pairing.json # wrong key: must be rejected as undecryptable
```

Other flags: `--sender "+41…"`. A pairing file is the QR JSON: `{"v":1,"relay":"https://ntfy.sh","topic":"…","key":"…"}`.

### Manual checklist

- [ ] Fresh start shows the Pairing tab; QR appears only after **Show QR code** and disappears when switching tabs.
- [ ] Badge shows **Connected**; tray tooltip shows the same status.
- [ ] Code with the terminal/Notepad focused (not allowlisted) → clipboard + toast; clipboard empty again after 30 s.
- [ ] Code with an allowlisted app focused → typed in, toast "Typed code into <exe>"; with *Press Enter* also submits.
- [ ] Stale, duplicate and wrong-key messages are rejected with a notice in the Codes tab.
- [ ] **Ctrl+Alt+Shift+O** types the latest code into the focused app.
- [ ] Reveal shows the code and re-masks after 10 s; Copy copies it; codes vanish after 5 minutes.
- [ ] Closing the window hides it to the tray; tray **Quit** exits and clears a pending clipboard code.
- [ ] Starting the app a second time only brings the first window to the front.
- [ ] Put the PC to sleep for a few minutes, wake it: the badge returns to **Connected** and new codes arrive.
- [ ] *Start with Windows* (installed build only): after sign-in the app starts hidden in the tray.

## Security model

- Codes are encrypted on the phone and decrypted only on the paired PC; ntfy.sh never sees plaintext or the key.
- Messages are rejected if they fail authentication, are stale (> 2 min) or are replays.
- OTPs and keys are never logged or written to disk; history lives in memory for 5 minutes.
- Auto-typing only happens into allowlisted applications and only codes of 4–10 letters/digits are accepted.
- **Anyone who can send you an SMS can make the PC type 4–10 letters/digits into an allowlisted app.** The Android app
  must only forward SMS from trusted senders; keep *Press Enter* off unless you need it.

## Limitations

- Windows does not allow typing into elevated (administrator) windows from a normal app; those get the clipboard.
- Letters are typed as Unicode keystrokes (digits as real keys). If a remote session drops letters, use the clipboard.
- Test autostart only with the installed build (a dev build registers its debug path) and turn it off before
  uninstalling — the uninstaller does not remove the autostart entry.
- After a restart, messages from up to ~2 minutes before the app started can still be delivered.
- The pairing is a Credential Manager entry with enterprise persistence (set by the `keyring` crate). With a domain
  roaming profile it follows you to other PCs, and if this app is installed there too it also receives your codes.
  Only install the app on PCs where you want your codes.

## Development

Prerequisites:

- Windows 10 or 11
- [Rust](https://rustup.rs/) stable (MSVC toolchain)
- Visual Studio Build Tools with **Desktop development with C++**
- Node.js LTS
- WebView2 runtime (preinstalled on Windows 11 and current Windows 10)

```powershell
npm install
npm run tauri dev       # run in development
npm run tauri build     # build the NSIS installer
npm run check           # frontend type-check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

## Roadmap

1. Android companion app (separate repo)
2. Later: Linux (Ubuntu) support

## Contributing with AI agents

See [`CLAUDE.md`](CLAUDE.md) for project conventions and the subagent workflow; agent definitions live in
[`.claude/agents/`](.claude/agents/).