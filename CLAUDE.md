# CLAUDE.md

Speak efficiently in user-facing messages to save tokens, but use clear, standard English for planning, code, structured
data, tool calls, and any other internal or technical outputs

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

**OTP Forwarder Desktop** — a small Windows tray app that receives SMS one-time passwords forwarded from the user's
Android phone and **auto-types them into the focused application** (Citrix Workspace, browser login pages, etc.).
It is a personal tool: favor simplicity and security over features and scale.

```
Android phone (SMS) ──AES-GCM encrypted──▶ ntfy.sh relay ──▶ this desktop app ──SendInput──▶ focused app (Citrix, …)
```

- The **Android companion app** (Kotlin, SMS only) lives in a separate repository. This repo is the **desktop** side.
- The relay only ever sees ciphertext. The shared key is exchanged once via a QR code shown by the desktop app.

> Status: v0.1 implemented (desktop side). Extend existing modules; keep the structure below.

## Tech & patterns (quick reference)

| Area             | Choice                                                                                   |
|------------------|------------------------------------------------------------------------------------------|
| Desktop shell    | **Tauri v2** (Rust, stable toolchain, MSVC target)                                       |
| Target OS        | **Windows 10 and Windows 11** only for now; Linux (Ubuntu) possibly later                |
| Frontend         | **Svelte 5** (runes) + **TypeScript** (strict) + **Vite**, plain Svelte (no SvelteKit)   |
| Styling          | **Pico.css** + small scoped component styles                                             |
| Transport        | **ntfy.sh** public relay, desktop subscribes outbound (no open ports, works behind NAT)  |
| Encryption       | **AES-256-GCM** end-to-end, 32-byte key shared via QR pairing                             |
| Secret storage   | `keyring` crate → Windows Credential Manager                                              |
| Windows APIs     | `windows` crate: `SendInput` (Unicode), `GetForegroundWindow`, process image name lookup |
| Tauri plugins    | notification, global-shortcut, autostart, single-instance (all used from Rust only); tray via core |
| Clipboard        | `arboard` (excluded from clipboard history and cloud sync)                               |
| Package manager  | npm                                                                                       |
| Not in scope     | LAN-direct delivery, notification-based OTPs, macOS, Linux (for now)                     |

Patterns:

- **OS-specific code behind traits** (`platform` module: input typing, foreground-window detection, autostart). Only a
  Windows implementation exists; never leak `windows` crate types outside `platform::windows`. This keeps a later
  Ubuntu port to a new `impl`.
- **Business logic is pure Rust** (crypto, message parsing, rules, dedupe/expiry) and unit-testable without Tauri or
  Windows.
- **Thin Tauri command layer**: `#[tauri::command]` functions only translate between IPC and core services.
- **Frontend is presentation only**: pairing QR, recent-codes list, settings/rules. All security-relevant logic stays in
  Rust.
- **Least privilege**: Tauri capabilities (`src-tauri/capabilities/`) grant only the commands/plugins actually used.

## Subagent workflow

For any non-trivial task, drive the work through subagents in this order. Steps in _(parentheses)_ are conditional.
**Feedback always loops back to the specific agent that owns the thing being changed and is never patched inline by
whichever agent happens to be running.** If sparring-partner (or anyone else) finds a gap in the plan or design,
re-invoke **planner** (and **senior-architect** too, if the gap is architectural) with that feedback — don't silently
work around it. If a reviewer finds an implementation problem, re-invoke **senior-software-engineer**; if it finds a
test-coverage gap, re-invoke **senior-test-engineer**. Keep looping a step until its reviewer/critic is satisfied before
moving on.

```
planner
  → (senior-architect)         ── fold its design back into the plan
  → sparring-partner           ── stress-test the plan
  → planner / senior-architect ── revise the plan/design if critique found gaps
  → senior-software-engineer   ── implement
  → code-reviewer              ── review the implementation
  → senior-software-engineer   ── address findings (loop until clean)
  → senior-test-engineer       ── add / strengthen tests
  → code-reviewer              ── 2nd pass: are any test types missing?
  → senior-test-engineer       ── address findings (loop until clean)
  → (final-code-reviewer)      ── last gate for high-risk changes
```

1. **planner** — turn the task into a concrete, sequenced plan.
2. **senior-architect** _(if it crosses module boundaries, changes the wire/pairing contract, adds a dependency, Tauri
   plugin or pattern, or has competing designs)_ — feed its output back into the plan.
3. **sparring-partner** — challenge assumptions, edge cases, failure modes, second-order effects, simpler alternatives.
4. **planner and/or senior-architect** _(whenever sparring surfaced gaps)_ — revise the plan/design; re-run
   sparring-partner if the plan is substantially different.
5. **senior-software-engineer** — implement the agreed plan.
6. **code-reviewer** — correctness, security, reliability, design.
7. **senior-software-engineer** _(whenever code-reviewer found issues)_ — address the findings; re-run code-reviewer if
   the fix is substantial.
8. **senior-test-engineer** — cover the new/changed code.
9. **code-reviewer** — second pass focused on test coverage: are unit / integration / edge-case / regression tests
   missing?
10. **senior-test-engineer** _(whenever this pass found gaps)_ — address the findings.
11. **final-code-reviewer** _(high-risk changes only: crypto/key handling, pairing, the relay wire format, keystroke
    injection / foreground-window rules, `unsafe`/FFI, Tauri capabilities, new dependencies, or large diffs)_.

## Implementing many user stories at once

When you are provided with many user stories, always do the following for the subagent workflow when creating plans and
implementations:

1. First, look at all user stories and ask any relevant questions you might have to improve the final plan
2. Then, create an overview of all user stories to implement so that progress can be tracked
3. Then, create a plan for the first user story doing the usual subagent workflow
4. Once the plan is fully ready, start the implementation of the first user story and at the same time, begin planning
   the next user story. You can plan two new user stories at most.
5. Do not implement different user stories at the same time! This can lead to editing the same files and code conflicts!
   Instead, once the plans of the next user stories are fully ready, wait until the first user story is fully
   implemented before starting the implementation of the next user story.
6. When the next user story is being implemented, you can start planning the user story after that
7. After the current user story has finished the engineering and review part and continues to the test part, you can
   start implementing the next user story already, since no changes to the business logic are expected at that point.
8. Once a user story is fully implemented, create a short and precise commit (see git log for examples) and push it

## Repository shape

```
otp-forwarder-desktop/
├── src/                       Svelte 5 + TS frontend (Vite root)
│   ├── lib/                   ipc.ts (typed invoke/listen wrappers), Recent/Pairing/Settings.svelte
│   ├── App.svelte
│   └── main.ts
├── src-tauri/                 Rust backend (Tauri v2)
│   ├── src/
│   │   ├── main.rs            entry point → lib::run()
│   │   ├── lib.rs             Tauri builder: plugins, tray, state, commands
│   │   ├── commands.rs        #[tauri::command] IPC layer (thin)
│   │   ├── pipeline.rs        AppState, relay → receive → deliver, recent list, clipboard clear
│   │   ├── relay.rs           ntfy subscription (JSON stream), reconnect/backoff, publish
│   │   ├── crypto.rs          AES-256-GCM encrypt/decrypt, key/topic generation
│   │   ├── pairing.rs         pairing JSON/QR, key persistence via keyring
│   │   ├── otp.rs             message model, validation, freshness, dedupe
│   │   ├── settings.rs        settings.json load/validate/atomic save
│   │   ├── rules.rs           foreground-app allowlist + per-app actions (Enter, delay)
│   │   └── platform/          trait (mod.rs) + windows.rs (SendInput, foreground window, clipboard)
│   ├── examples/otpfwd-send.rs  test sender CLI (phone stand-in; never bundled)
│   ├── build.rs               embeds the app manifest into all targets (tauri#13419) + command allowlist
│   ├── capabilities/          Tauri permission sets
│   ├── icons/
│   ├── Cargo.toml
│   └── tauri.conf.json
├── docs/wire-contract.md      wire contract v1 + threat model (authoritative)
├── .claude/agents/            subagent definitions
├── package.json
└── vite.config.ts
```

## Commands

| Task                    | Command                                                         |
|-------------------------|-----------------------------------------------------------------|
| Install JS deps         | `npm install`                                                   |
| Run app (dev)           | `npm run tauri dev`                                             |
| Build installer         | `npm run tauri build` (NSIS, bootstraps WebView2 on Win 10)     |
| Test sender CLI         | `cargo run --manifest-path src-tauri/Cargo.toml --example otpfwd-send -- --code 123456` |
| Frontend type-check     | `npm run check` (svelte-check)                                  |
| Rust tests              | `cargo test --manifest-path src-tauri/Cargo.toml`               |
| Rust lint (must pass)   | `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` |
| Rust format             | `cargo fmt --manifest-path src-tauri/Cargo.toml`                |

Prerequisites: Rust stable (`x86_64-pc-windows-msvc`), Visual Studio Build Tools with "Desktop development with C++",
Node.js LTS, WebView2 runtime (preinstalled on Windows 11 and current Windows 10).

## Commit message convention

Write commit messages in **imperative present tense**, short and to the point.
Start with a capital letter, no period at the end.

Examples:

- ✅ `Add ntfy relay subscriber`
- ✅ `Fix nonce reuse in AES-GCM encryption`
- ✅ `Refactor foreground window detection behind platform trait`
- ✅ `Update Cargo.toml with keyring dependency`
- ❌ `added relay subscriber` (lowercase)
- ❌ `Adds relay subscriber.` (wrong tense, period)
- ❌ `chore: Add relay subscriber` (prefix tag not used)

This keeps the git log readable and uniform when many agents touch the repo.

## Backend (`src-tauri/`)

- Rust 2024 edition, stable toolchain. `rustfmt` default style; `clippy -D warnings` must pass.
- Async runtime: Tokio (Tauri's runtime). HTTP client: `reqwest` (rustls) for the ntfy stream.
- If test binaries fail with `STATUS_ENTRYPOINT_NOT_FOUND`, the build cache is stale: `cargo clean -p otp-forwarder-desktop`.
- Errors: `thiserror` for module error types; map to serializable error strings only at the command layer. No
  `unwrap()`/`expect()` outside tests and provably infallible startup code.
- `unsafe` only inside `platform::windows`, minimal and each block with a `// SAFETY:` comment.
- Secrets: keys live in `keyring`, are held in memory as `zeroize`-able types, and are **never logged**. OTP codes are
  never logged or written to disk; history is in-memory and short-lived.

### Architecture & conventions

- Data flow: `relay` receives ciphertext → `crypto` decrypts → `otp` validates (version, freshness, dedupe) → `rules`
  decides the action for the current foreground app → `platform` types it, or falls back to clipboard + toast → event
  emitted to the frontend.
- Auto-type only when the foreground process matches the allowlist (e.g. `wfica32.exe`, `SelfService.exe`); otherwise
  clipboard + notification. Clipboard is cleared after a timeout.
- Known Windows limitation: `SendInput` cannot type into elevated (admin) windows from a non-elevated process (UIPI).
- Keep the relay reconnecting with backoff; the app must survive sleep/resume and network changes.

## Frontend (`src/`)

- Svelte 5 runes (`$state`, `$derived`, `$effect`, `$props`); TypeScript `strict`.
- Screens: **Pairing** (QR code + status), **Recent codes** (masked, auto-expiring), **Settings** (app allowlist,
  per-app Enter/delay, autostart, clipboard timeout).
- Talk to Rust only through typed wrappers in `src/lib/` around `invoke`/`listen`; no direct plugin calls scattered in
  components.
- Keep dependencies minimal; no UI framework beyond Pico.css.

## Cross-cutting contract (desktop ↔ Android)

Wire contract version 1 — the authoritative spec is [`docs/wire-contract.md`](docs/wire-contract.md) (limits, test
vector, threat model). Any change goes through **senior-architect**, must be mirrored in the Android repo and bump `v`.

- **Pairing QR payload** (JSON, shown by desktop, scanned by phone):
  `{ "v": 1, "relay": "https://ntfy.sh", "topic": "<random 128-bit, base64url>", "key": "<32 bytes, base64url>" }`
- **Message** (published by phone to `POST {relay}/{topic}`): body is `base64url(nonce[12] || ciphertext || tag)`,
  AES-256-GCM, AAD = `"otpfwd/v1/" + topic`.
- **Plaintext** (JSON): `{ "v": 1, "id": "<16–64 [A-Za-z0-9_-]>", "code": "<4–10 alphanumerics>", "sender": "+41…", "ts": <unix ms> }`
- Desktop subscribes via `GET {relay}/{topic}/json` (streaming). It rejects messages that fail decryption, have an
  unknown `v`, are older than 2 minutes (or > 30 s in the future), or reuse an `id` seen in the last 10 minutes.
- The topic name is a secret-ish identifier, not a security boundary; confidentiality and authenticity come from
  AES-GCM.
