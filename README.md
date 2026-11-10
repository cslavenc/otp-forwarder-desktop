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

### Pairing

Pairing is a one-time exchange of a shared secret; there is no account or server-side registration.

- On first start the app generates a random **topic** (128 bits) and a random **key** (256 bits) and stores both in
  Windows Credential Manager.
- The QR code is just this JSON: `{"v":1,"relay":"https://ntfy.sh","topic":"…","key":"…"}`. It is only rendered
  when you click **Show QR code**, and it is discarded when you switch tabs or close the window, because it contains
  the key.
- The topic is the ntfy "channel" both sides use. It is hard to guess but not a security boundary: whoever knows the
  topic can read ciphertext or post junk, but cannot decrypt or forge a code without the key.
- **Re-pair** throws both away and creates new ones; old phones stop working until they scan again.

### What happens to one code

```
phone                                   ntfy.sh                    desktop (tray app)
─────                                   ───────                    ──────────────────
SMS "Your code is 482913"
 └ extract code
 └ JSON {v,id,code,sender,ts}
 └ AES-256-GCM encrypt (random nonce)
 └ POST https://ntfy.sh/<topic> ───────▶ stores & fans out ───────▶ GET /<topic>/json (long-lived stream)
                                                                    1. decrypt + authenticate
                                                                    2. validate
                                                                    3. decide: type or copy
                                                                    4. deliver, notify, update UI
```

1. **Decrypt and authenticate.** The body is `base64url(nonce ‖ ciphertext ‖ tag)`. The topic is bound into the
   authentication data, so a message copied to another topic fails. Anything that does not decrypt is dropped.
2. **Validate** (in this order, see [`otp.rs`](src-tauri/src/otp.rs)):
   - known protocol version (`v: 1`);
   - the code is 4–10 letters/digits;
   - the phone's timestamp is at most **2 minutes** old and at most 30 s in the future (keep both clocks on
     automatic time);
   - the message `id` was not seen in the last 10 minutes (replay protection).

   Rejected messages appear as a notice in the **Codes** tab; their content is never shown.
3. **Decide** what to do based on the **focused window** at that moment:

   | Focused window                                         | Result                                    |
   |--------------------------------------------------------|-------------------------------------------|
   | Its exe is on the allowlist and the rule is enabled    | **Typed in** (then Enter, if configured)  |
   | Not on the allowlist, or *Auto-type* is off            | Clipboard + notification                  |
   | Runs as administrator                                  | Clipboard (Windows blocks typing into it) |
   | OTP Forwarder itself, or nothing focused               | Clipboard                                 |

   Matching is by process file name only (e.g. `wfica32.exe`), case-insensitive.
4. **Deliver.**
   - **Typing** uses Windows `SendInput`, one character at a time with the rule's delay (default 15 ms). Before every
     keystroke it re-checks that the same window still has focus and that you are not holding Shift/Ctrl/Alt/Win.
     If either check fails it stops, never presses Enter, and falls back to the clipboard. Digits are sent as real
     key presses, which remote sessions like Citrix handle best.
   - **Clipboard** fallback copies the code and clears it again after 30 s (configurable), or immediately when you
     quit, but only if the clipboard still holds that code.
   - The code is added to **Codes** (masked, kept in memory for 5 minutes) and a notification says what happened.
     Notifications never contain the code.

**Missed it?** Focus the right field and press **Ctrl+Alt+Shift+O** to type the latest code. This works in any
non-admin app, not just allowlisted ones, because you asked for it explicitly. Or use **Copy** in the Codes tab.

### Staying connected

The app keeps one streaming HTTPS connection to ntfy.sh open. If it drops (Wi-Fi change, sleep/resume, ntfy restart)
it reconnects with backoff (1 s doubling up to 60 s) and asks ntfy for messages since shortly before the last one it
saw, so nothing sent during a short outage is lost; repeats are filtered by the replay check. The badge in the window
and the tray tooltip show **Connected**, **Connecting** or **Reconnecting in N s: reason**.

### Finding an app's exe name for the allowlist

The easiest way: in **Settings**, click **Detect app…** and switch to the app within 5 seconds; its name lands in the
*Add an app* field and you confirm with **Add**. This reports the process that really owns the window (for Citrix the
session window is usually `wfica32.exe`, `CDViewer.exe` for some desktop viewers, and the Workspace login window is
`SelfService.exe`). Store apps all report `ApplicationFrameHost.exe`, so allowlisting it covers *every* Store app.

Alternatively, **Browse…** to the app's `.exe` (not a shortcut), or open **Task Manager → Details** and find the row
that uses CPU when you interact with it; the *Name* column is the name. You can type the name with or without `.exe`
(`notepad` works), or paste the full path of the exe. For a browser it would be `chrome.exe`, `msedge.exe`, etc. Note
that allowlisting a browser means codes are typed into *any* focused page.

## Features

- QR-code pairing, end-to-end AES-256-GCM encryption (the relay only sees ciphertext); key in Windows Credential Manager
- Rejects messages that fail authentication, are older than 2 minutes, come from the future, or are replays
- Auto-type into allowlisted apps (default: `wfica32.exe`, `CDViewer.exe`, `SelfService.exe`) via `SendInput`,
  keyboard-layout aware; clipboard + toast fallback everywhere else
- Per-app rules: press Enter after typing, delay between keystrokes
- Global hotkey (default **Ctrl+Alt+Shift+O**) types the latest code into the focused app; change it in **Settings** by
  clicking the shortcut and pressing the new keys (at least two modifiers), or Backspace to turn it off
- Recent codes (masked, in memory only, gone after 5 minutes); clipboard auto-clear, also on exit
- Tray icon with relay status; closing the window hides it; optional start with Windows
- Windows 10 and Windows 11

## Install and first run

1. Build the installer with `npm run release` (see [Building the exe files](#building-the-exe-files)) and run
   `release\OTP Forwarder_0.1.0_x64-setup.exe`.
2. Start **OTP Forwarder**. On first start it creates a pairing and opens the **Pairing** tab.
3. Click **Show QR code** and scan it with the Android app. The badge in the top-right should say **Connected**.
4. Optional: in **Settings**, enable *Start with Windows* and adjust the allowlist; changes are saved automatically.

### Getting the app onto your phone

The phone side is the separate `otp-forwarder-android` repo. From its folder, with the phone connected via USB or
wireless debugging, run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-install.ps1
```

It builds the release APK and installs it (or leaves `release\otp-forwarder.apk` to copy manually). See that repo's
README for phone setup and pairing.

Settings are stored in `%APPDATA%\ch.cslavenc.otpforwarder\settings.json`; the pairing key in Windows Credential
Manager (generic credential `ch.cslavenc.otpforwarder`). **Re-pair** creates a new key and topic — the phone must scan
again.

## Testing

You can test the whole path (relay → decryption → typing) without the Android app.

**From the app (no tools needed):** open **Codes → Send test code**, then within 5 seconds click into the target field
(e.g. a Citrix login prompt, or Notepad: add it temporarily via **Settings → Detect app…** while Notepad is in front).
The app publishes an
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
- [ ] Add an app in **Settings**, use tray **Quit**, start again → the app is still listed.
- [ ] While another program holds the hotkey (and notifications are off), Settings still shows a hotkey warning; once
      it is released, changing any setting makes the hotkey work.
- [ ] **Detect app…** with Notepad, a Citrix session, an admin `cmd` (refused) and OTP Forwarder itself (refused).
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
- The replay list lives in memory: right after the app restarts, a captured message that is still under 2 minutes old
  could be delivered once more.
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

### Building the exe files

One command builds everything (first run takes several minutes; later runs are faster):

```powershell
npm run release
```

It runs [`scripts/build-release.ps1`](scripts/build-release.ps1), which stops at the first failing step:

1. `npm ci`: installs the exact JS dependencies from `package-lock.json`
2. `npm run check`, `cargo test` and `cargo clippy`: a broken build is never packaged
3. `npm run tauri build`: compiles the release app and the NSIS installer
4. copies the results into `release\` (emptied first, git-ignored):

| File                                     | Use                                                                 |
|------------------------------------------|---------------------------------------------------------------------|
| `OTP Forwarder_<version>_x64-setup.exe`  | Installer: Start-menu entry, uninstaller, installs WebView2 if missing |
| `OTP Forwarder_<version>_x64-portable.exe` | The app itself, no install; needs the WebView2 runtime              |

The version comes from `src-tauri/tauri.conf.json`. Without npm you can run the script directly:
`powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1`. The exe files are unsigned, so Windows
SmartScreen may warn on first start (**More info → Run anyway**).

Code map (all security-relevant logic is in Rust; the Svelte UI only displays and edits settings):

| Path                                 | Responsibility                                                              |
|--------------------------------------|-----------------------------------------------------------------------------|
| `src-tauri/src/crypto.rs`            | AES-256-GCM seal/open, key and topic generation                             |
| `src-tauri/src/pairing.rs`           | Pairing JSON, QR rendering, Credential Manager storage                      |
| `src-tauri/src/relay.rs`             | ntfy subscribe stream, reconnect/backoff, publish (test code)               |
| `src-tauri/src/otp.rs`               | Plaintext parsing and validation: version, code, freshness, replay          |
| `src-tauri/src/rules.rs`             | Allowlist matching → type or copy                                           |
| `src-tauri/src/settings.rs`          | `settings.json` load/validate/save                                          |
| `src-tauri/src/platform/`            | OS boundary: typing (`SendInput`), focused window, clipboard (Windows impl) |
| `src-tauri/src/pipeline.rs`          | Glues it together: relay → decrypt → validate → deliver → events/toasts     |
| `src-tauri/src/commands.rs`, `lib.rs`| Thin Tauri commands; app setup, tray, hotkey, autostart                     |
| `src-tauri/examples/otpfwd-send.rs`  | Test sender that plays the phone's role                                     |
| `src/`                               | Svelte 5 UI: `Recent`, `Pairing`, `Settings`; typed IPC in `src/lib/ipc.ts` |

## Roadmap

1. Android companion app (separate repo)
2. Later: Linux (Ubuntu) support

## Contributing with AI agents

See [`CLAUDE.md`](CLAUDE.md) for project conventions and the subagent workflow; agent definitions live in
[`.claude/agents/`](.claude/agents/).