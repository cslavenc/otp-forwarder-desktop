//! App state and the OTP pipeline: relay events → decrypt/validate → type or copy → UI events.
//! Codes are never logged; toasts carry neither the code nor the sender.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::async_runtime::{self, JoinHandle};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::mpsc;
use tokio::time::sleep;
use zeroize::Zeroizing;

use crate::crypto;
use crate::otp::{self, Dedupe};
use crate::pairing::{DEFAULT_RELAY, Pairing};
use crate::platform::{Foreground, Platform};
use crate::relay::{self, RelayEvent, RelayStatus};
use crate::rules::{self, Action};
use crate::settings::{DEFAULT_DELAY_MS, Settings};

pub const RECENT_TTL_MS: i64 = 5 * 60_000;
const MAX_RECENT: usize = 20;
/// Gives the user time to focus the target field after clicking "Send test code".
const TEST_CODE_DELAY: Duration = Duration::from_secs(5);
const TEST_SENDER: &str = "Test";
pub const TRAY_ID: &str = "main";
const APP_NAME: &str = "OTP Forwarder";
/// The hotkey types into any app; without a matching rule it uses these.
const HOTKEY_DEFAULT_ACTION: Action = Action {
    press_enter: false,
    delay_ms: DEFAULT_DELAY_MS,
};

pub const EVENT_RELAY_STATUS: &str = "relay-status";
pub const EVENT_RECENT: &str = "recent";
pub const EVENT_REJECTED: &str = "rejected";

pub enum PairingState {
    Paired(Pairing),
    /// The stored pairing could not be read (or created); kept untouched until re-pair.
    Unpaired {
        detail: String,
    },
}

pub struct AppState {
    pub(crate) platform: Box<dyn Platform>,
    pub(crate) http: reqwest::Client,
    pub(crate) settings_path: PathBuf,
    relay_tx: mpsc::Sender<RelayEvent>,
    inner: Mutex<Inner>,
    /// Serializes typing so two deliveries never interleave keystrokes.
    typing: Mutex<()>,
    /// Serializes clipboard writes with arming/running their clear, so clears follow write
    /// order. Taken before `inner`.
    clipboard: tokio::sync::Mutex<()>,
}

pub(crate) struct Inner {
    pub(crate) pairing: PairingState,
    pub(crate) settings: Settings,
    /// Set when the settings file exists but could not be read: saving would overwrite it.
    pub(crate) settings_error: Option<String>,
    /// `None` until a relay was started (i.e. while unpaired).
    pub(crate) relay: Option<RelayStatus>,
    pub(crate) recent: RecentCodes,
    dedupe: Dedupe,
    clipboard_clear: ClipboardClear,
    relay_task: Option<JoinHandle<()>>,
}

impl AppState {
    pub fn new(
        platform: Box<dyn Platform>,
        http: reqwest::Client,
        settings_path: PathBuf,
        relay_tx: mpsc::Sender<RelayEvent>,
        pairing: PairingState,
        settings: Settings,
        settings_error: Option<String>,
    ) -> Self {
        Self {
            platform,
            http,
            settings_path,
            relay_tx,
            inner: Mutex::new(Inner {
                pairing,
                settings,
                settings_error,
                relay: None,
                recent: RecentCodes::default(),
                dedupe: Dedupe::default(),
                clipboard_clear: ClipboardClear::default(),
                relay_task: None,
            }),
            typing: Mutex::new(()),
            clipboard: tokio::sync::Mutex::new(()),
        }
    }

    /// Never hold the guard across `.await` or while calling into Tauri.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        // The state stays consistent even if a holder panicked; keep serving it.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The code awaiting its clipboard clear; only the newest armed timer may clear it.
#[derive(Default)]
struct ClipboardClear {
    seq: u64,
    pending: Option<Zeroizing<String>>,
}

impl ClipboardClear {
    fn arm(&mut self, code: Zeroizing<String>) -> u64 {
        self.seq += 1;
        self.pending = Some(code);
        self.seq
    }

    /// The pending code if timer `seq` is still the newest.
    fn due(&self, seq: u64) -> Option<Zeroizing<String>> {
        self.pending.clone().filter(|_| self.seq == seq)
    }

    fn take_if(&mut self, seq: u64) -> Option<Zeroizing<String>> {
        if self.seq == seq {
            self.pending.take()
        } else {
            None
        }
    }

    fn take(&mut self) -> Option<Zeroizing<String>> {
        self.pending.take()
    }
}

/// How a received code reached the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Delivery {
    Pending,
    Typed { target: String },
    Copied { reason: String },
    Failed { reason: String },
}

struct Recent {
    id: String,
    code: Zeroizing<String>,
    sender: String,
    received_ms: i64,
    delivery: Delivery,
}

/// What the UI may see of a recent code: masked, never the code itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecentView {
    pub id: String,
    pub masked: String,
    pub sender: String,
    pub received_ms: i64,
    pub expires_ms: i64,
    pub delivery: Delivery,
}

/// Recently received codes, newest first; entries expire after [`RECENT_TTL_MS`].
#[derive(Default)]
pub(crate) struct RecentCodes(Vec<Recent>);

impl RecentCodes {
    fn push(&mut self, id: &str, code: Zeroizing<String>, sender: &str, now_ms: i64) {
        self.prune(now_ms);
        self.0.insert(
            0,
            Recent {
                id: id.to_owned(),
                code,
                sender: sender.to_owned(),
                received_ms: now_ms,
                delivery: Delivery::Pending,
            },
        );
        self.0.truncate(MAX_RECENT);
    }

    fn prune(&mut self, now_ms: i64) {
        self.0
            .retain(|r| r.received_ms.saturating_add(RECENT_TTL_MS) > now_ms);
    }

    pub(crate) fn code(&mut self, id: &str, now_ms: i64) -> Option<Zeroizing<String>> {
        self.prune(now_ms);
        self.0.iter().find(|r| r.id == id).map(|r| r.code.clone())
    }

    fn latest(&mut self, now_ms: i64) -> Option<Zeroizing<String>> {
        self.prune(now_ms);
        self.0.first().map(|r| r.code.clone())
    }

    fn set_delivery(&mut self, id: &str, delivery: Delivery) {
        if let Some(r) = self.0.iter_mut().find(|r| r.id == id) {
            r.delivery = delivery;
        }
    }

    pub(crate) fn views(&mut self, now_ms: i64) -> Vec<RecentView> {
        self.prune(now_ms);
        self.0
            .iter()
            .map(|r| RecentView {
                id: r.id.clone(),
                masked: "•".repeat(r.code.chars().count()),
                sender: r.sender.clone(),
                received_ms: r.received_ms,
                expires_ms: r.received_ms.saturating_add(RECENT_TTL_MS),
                delivery: r.delivery.clone(),
            })
            .collect()
    }
}

/// Consumes relay events one at a time until the relay sender side is dropped.
pub async fn run(app: AppHandle, mut rx: mpsc::Receiver<RelayEvent>) {
    while let Some(event) = rx.recv().await {
        match event {
            RelayEvent::Status(status) => on_status(&app, status),
            RelayEvent::Message { body, .. } => on_message(&app, &body).await,
        }
    }
}

/// (Re)starts the relay subscription for the current pairing; stops it while unpaired.
pub fn restart_relay(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut inner = state.lock();
    if let Some(task) = inner.relay_task.take() {
        task.abort();
    }
    let PairingState::Paired(pairing) = &inner.pairing else {
        return;
    };
    let task = async_runtime::spawn(relay::subscribe(
        state.http.clone(),
        pairing.relay.clone(),
        pairing.topic.clone(),
        state.relay_tx.clone(),
    ));
    inner.relay_task = Some(task);
    inner.relay = Some(RelayStatus::Connecting);
}

/// Generates and stores a new pairing, then restarts the relay. The old phone pairing stops
/// working.
pub fn repair(app: &AppHandle) -> Result<(), String> {
    let pairing = Pairing::generate(DEFAULT_RELAY).map_err(|e| e.to_string())?;
    pairing.save().map_err(|e| e.to_string())?;
    app.state::<AppState>().lock().pairing = PairingState::Paired(pairing);
    restart_relay(app);
    update_tray(app);
    Ok(())
}

fn on_status(app: &AppHandle, status: RelayStatus) {
    app.state::<AppState>().lock().relay = Some(status.clone());
    let _ = app.emit(EVENT_RELAY_STATUS, &status);
    update_tray(app);
}

pub(crate) fn update_tray(app: &AppHandle) {
    let tooltip = tray_tooltip(app.state::<AppState>().lock().relay.as_ref());
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

pub(crate) fn tray_tooltip(relay: Option<&RelayStatus>) -> String {
    let status = match relay {
        None => "not paired".to_owned(),
        Some(RelayStatus::Connecting) => "connecting".to_owned(),
        Some(RelayStatus::Connected) => "connected".to_owned(),
        Some(RelayStatus::Reconnecting { in_secs, .. }) => format!("reconnecting in {in_secs} s"),
    };
    format!("{APP_NAME} – {status}")
}

async fn on_message(app: &AppHandle, body: &str) {
    let now = now_ms();
    let received = {
        let state = app.state::<AppState>();
        let mut guard = state.lock();
        let inner = &mut *guard;
        let PairingState::Paired(pairing) = &inner.pairing else {
            return;
        };
        otp::receive(&pairing.key, &pairing.topic, body, now, &mut inner.dedupe)
    };
    let mut msg = match received {
        Ok(msg) => msg,
        Err(reject) => {
            debug_log(&format!("message rejected: {reject}"));
            let _ = app.emit(EVENT_REJECTED, &reject);
            return;
        }
    };
    let code = Zeroizing::new(std::mem::take(&mut msg.code));
    app.state::<AppState>()
        .lock()
        .recent
        .push(&msg.id, code.clone(), &msg.sender, now);
    emit_recent(app);
    schedule_recent_expiry(app);

    let delivery = auto_deliver(app, code).await;
    app.state::<AppState>()
        .lock()
        .recent
        .set_delivery(&msg.id, delivery);
    emit_recent(app);
}

pub(crate) fn emit_recent(app: &AppHandle) {
    let views = app.state::<AppState>().lock().recent.views(now_ms());
    let _ = app.emit(EVENT_RECENT, views);
}

fn schedule_recent_expiry(app: &AppHandle) {
    let app = app.clone();
    async_runtime::spawn(async move {
        sleep(Duration::from_millis(RECENT_TTL_MS.unsigned_abs())).await;
        emit_recent(&app);
    });
}

async fn auto_deliver(app: &AppHandle, code: Zeroizing<String>) -> Delivery {
    let state = app.state::<AppState>();
    let foreground = state.platform.foreground();
    let (decision, notify) = {
        let inner = state.lock();
        (
            auto_type_target(&inner.settings, foreground),
            inner.settings.notifications,
        )
    };
    deliver(app, code, decision, notify).await
}

/// Hotkey "type last code": explicit user intent, so any app except our own (or an elevated
/// one) is a valid target.
pub fn on_hotkey(app: &AppHandle) {
    let app = app.clone();
    async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        // Pressing the hotkey aborts an in-flight auto-type (modifiers held); retyping the full
        // code into the half-filled field would leave a doubled code, so refuse instead.
        if matches!(state.typing.try_lock(), Err(TryLockError::WouldBlock)) {
            let notify = state.lock().settings.notifications;
            toast(&app, notify, "A code is being typed; try again");
            return;
        }
        let (code, notify) = {
            let mut inner = state.lock();
            (inner.recent.latest(now_ms()), inner.settings.notifications)
        };
        let Some(code) = code else {
            toast(&app, notify, "No recent code to type");
            return;
        };
        let foreground = state.platform.foreground();
        let decision = hotkey_target(&state.lock().settings, foreground);
        deliver(&app, code, decision, notify).await;
    });
}

/// Types into `decision`'s target, or falls back to the clipboard with the reason.
async fn deliver(
    app: &AppHandle,
    code: Zeroizing<String>,
    decision: Result<(Foreground, Action), String>,
    notify: bool,
) -> Delivery {
    let reason = match decision {
        Ok((target, action)) => match type_code(app, target.clone(), code.clone(), action).await {
            Ok(()) => {
                toast(app, notify, &format!("Typed code into {}", target.exe));
                return Delivery::Typed { target: target.exe };
            }
            Err(e) => format!("typing failed: {e}"),
        },
        Err(reason) => reason,
    };
    match copy_secret(app, code).await {
        Ok(()) => {
            toast(app, notify, &format!("Code copied to clipboard ({reason})"));
            Delivery::Copied { reason }
        }
        Err(e) => {
            let reason = format!("{reason}; copying failed: {e}");
            toast(app, notify, &format!("Could not deliver code: {reason}"));
            Delivery::Failed { reason }
        }
    }
}

fn usable_target(foreground: Option<Foreground>) -> Result<Foreground, String> {
    let fg = foreground.ok_or("no focused app")?;
    if fg.own {
        return Err("OTP Forwarder is focused".into());
    }
    if fg.elevated {
        return Err(format!("{} runs as administrator", display_exe(&fg)));
    }
    Ok(fg)
}

fn auto_type_target(
    settings: &Settings,
    foreground: Option<Foreground>,
) -> Result<(Foreground, Action), String> {
    let fg = usable_target(foreground)?;
    if !settings.auto_type {
        return Err("auto-type is off".into());
    }
    match rules::decide(settings, Some(&fg.exe)) {
        Some(action) => Ok((fg, action)),
        None => Err(format!("no auto-type rule for {}", display_exe(&fg))),
    }
}

fn hotkey_target(
    settings: &Settings,
    foreground: Option<Foreground>,
) -> Result<(Foreground, Action), String> {
    let fg = usable_target(foreground)?;
    let action = rules::find_rule(settings, &fg.exe).unwrap_or(HOTKEY_DEFAULT_ACTION);
    Ok((fg, action))
}

fn display_exe(fg: &Foreground) -> &str {
    if fg.exe.is_empty() {
        "the focused app"
    } else {
        &fg.exe
    }
}

async fn type_code(
    app: &AppHandle,
    target: Foreground,
    code: Zeroizing<String>,
    action: Action,
) -> Result<(), String> {
    let app = app.clone();
    async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _typing = state.typing.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .platform
            .type_text(&target, &code, action.press_enter, action.delay_ms)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Copies `code` (excluded from clipboard history) and schedules its clear.
pub(crate) async fn copy_secret(app: &AppHandle, code: Zeroizing<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _clipboard = state.clipboard.lock().await;
    let handle = app.clone();
    let copied = code.clone();
    async_runtime::spawn_blocking(move || handle.state::<AppState>().platform.copy_secret(&copied))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let (secs, seq) = {
        let mut inner = state.lock();
        let secs = inner.settings.clipboard_clear_secs;
        if secs == 0 {
            return Ok(());
        }
        (secs, inner.clipboard_clear.arm(code))
    };
    let app = app.clone();
    async_runtime::spawn(async move {
        sleep(Duration::from_secs(secs.into())).await;
        let state = app.state::<AppState>();
        let _clipboard = state.clipboard.lock().await;
        let Some(code) = state.lock().clipboard_clear.due(seq) else {
            return;
        };
        // Clear before dropping `pending`, so a quit meanwhile still finds (and clears) it.
        let handle = app.clone();
        let _ = async_runtime::spawn_blocking(move || clear_clipboard(&handle, &code)).await;
        state.lock().clipboard_clear.take_if(seq);
    });
    Ok(())
}

/// Runs a pending clipboard clear now (on exit). Clearing is idempotent, so racing a timer
/// is harmless.
pub fn clear_clipboard_now(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let pending = state.lock().clipboard_clear.take();
    if let Some(code) = pending {
        clear_clipboard(app, &code);
    }
}

fn clear_clipboard(app: &AppHandle, code: &str) {
    if let Err(e) = app.state::<AppState>().platform.clear_if_equals(code) {
        debug_log(&format!("clipboard clear failed: {e}"));
    }
}

/// Publishes a random 6-digit code to our own pairing after [`TEST_CODE_DELAY`], so it travels
/// the full relay → decrypt → deliver path. Returns once published.
pub async fn send_test_code(app: &AppHandle) -> Result<(), String> {
    let pairing = match &app.state::<AppState>().lock().pairing {
        PairingState::Paired(pairing) => pairing.clone(),
        PairingState::Unpaired { .. } => return Err("not paired".into()),
    };
    sleep(TEST_CODE_DELAY).await;
    let n = getrandom::u32().map_err(|e| e.to_string())?;
    let code = Zeroizing::new(format!("{:06}", n % 1_000_000));
    let id = otp::new_id().map_err(|e| e.to_string())?;
    let json = otp::encode_plaintext(&id, &code, TEST_SENDER, now_ms());
    let body = crypto::encrypt(&pairing.key, &pairing.topic, json.as_bytes())
        .map_err(|e| e.to_string())?;
    let http = app.state::<AppState>().http.clone();
    relay::publish(&http, &pairing.relay, &pairing.topic, &body)
        .await
        .map_err(|e| e.to_string())
}

/// `""` → no hotkey.
pub fn parse_hotkey(hotkey: &str) -> Result<Option<Shortcut>, String> {
    if hotkey.is_empty() {
        return Ok(None);
    }
    hotkey
        .parse()
        .map(Some)
        .map_err(|e| format!("invalid hotkey: {e}"))
}

/// Replaces the registered hotkey; on failure the old one is restored.
pub fn swap_hotkey(
    app: &AppHandle,
    old: Option<Shortcut>,
    new: Option<Shortcut>,
) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    // An unchanged hotkey whose earlier registration failed is retried, not skipped.
    if old == new && new.is_none_or(|s| shortcuts.is_registered(s)) {
        return Ok(());
    }
    if let Some(old) = old {
        let _ = shortcuts.unregister(old);
    }
    let Some(new) = new else {
        return Ok(());
    };
    shortcuts.register(new).map_err(|e| {
        if let Some(old) = old {
            let _ = shortcuts.register(old);
        }
        format!("cannot register hotkey (in use by another app?): {e}")
    })
}

pub(crate) fn toast(app: &AppHandle, enabled: bool, body: &str) {
    if !enabled {
        return;
    }
    let shown = app
        .notification()
        .builder()
        .title(APP_NAME)
        .body(body)
        .show();
    if let Err(e) = shown {
        debug_log(&format!("toast failed: {e}"));
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Debug builds only; callers never pass codes, keys or bodies.
pub(crate) fn debug_log(message: &str) {
    if cfg!(debug_assertions) {
        eprintln!("[otpfwd] {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{DEFAULT_HOTKEY, Rule};

    const NOW: i64 = 1_700_000_000_000;

    fn fg(exe: &str, own: bool, elevated: bool) -> Option<Foreground> {
        Some(Foreground {
            exe: exe.to_owned(),
            own,
            elevated,
            window: 1,
            thread: 1,
        })
    }

    fn code(c: &str) -> Zeroizing<String> {
        Zeroizing::new(c.to_owned())
    }

    fn action_of(decision: Result<(Foreground, Action), String>) -> Action {
        decision.unwrap().1
    }

    #[test]
    fn recent_codes_are_newest_first_masked_and_capped() {
        let mut recent = RecentCodes::default();
        recent.push("a", code("1234"), "Bank", NOW);
        recent.push("b", code("AbC123"), "", NOW + 1);
        let views = recent.views(NOW + 1);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].id, "b");
        assert_eq!(views[0].masked, "••••••");
        assert_eq!(views[1].masked, "••••");
        assert_eq!(views[1].sender, "Bank");
        assert_eq!(views[1].expires_ms, NOW + RECENT_TTL_MS);
        assert_eq!(views[1].delivery, Delivery::Pending);
        let json = serde_json::to_string(&views).unwrap();
        assert!(!json.contains("1234") && !json.contains("AbC123"), "{json}");

        for i in 0..MAX_RECENT {
            recent.push(&format!("x{i}"), code("1111"), "", NOW + 2);
        }
        assert_eq!(recent.views(NOW + 2).len(), MAX_RECENT);
        assert!(recent.code("a", NOW + 2).is_none());
    }

    #[test]
    fn recent_codes_expire_after_ttl() {
        let mut recent = RecentCodes::default();
        recent.push("old", code("1111"), "", NOW);
        recent.push("new", code("2222"), "", NOW + 1000);
        let last_valid = NOW + RECENT_TTL_MS - 1;
        assert_eq!(recent.code("old", last_valid).unwrap().as_str(), "1111");
        assert_eq!(recent.latest(last_valid).unwrap().as_str(), "2222");

        assert!(recent.code("old", NOW + RECENT_TTL_MS).is_none());
        assert_eq!(recent.views(NOW + RECENT_TTL_MS).len(), 1);
        assert!(recent.latest(NOW + 1000 + RECENT_TTL_MS).is_none());
    }

    #[test]
    fn delivery_is_updated_by_id() {
        let mut recent = RecentCodes::default();
        recent.push("a", code("1234"), "", NOW);
        let typed = Delivery::Typed {
            target: "wfica32.exe".into(),
        };
        recent.set_delivery("a", typed.clone());
        recent.set_delivery("missing", Delivery::Pending);
        assert_eq!(recent.views(NOW)[0].delivery, typed);
        assert_eq!(
            serde_json::to_string(&typed).unwrap(),
            r#"{"kind":"typed","target":"wfica32.exe"}"#
        );
    }

    #[test]
    fn auto_type_needs_a_usable_target_and_a_rule() {
        let s = Settings::default();
        let action = action_of(auto_type_target(&s, fg("WFICA32.EXE", false, false)));
        assert_eq!(action.delay_ms, DEFAULT_DELAY_MS);

        assert!(auto_type_target(&s, None).is_err());
        let own = auto_type_target(&s, fg("otp-forwarder-desktop.exe", true, false));
        assert_eq!(own.unwrap_err(), "OTP Forwarder is focused");
        let elevated = auto_type_target(&s, fg("wfica32.exe", false, true));
        assert_eq!(elevated.unwrap_err(), "wfica32.exe runs as administrator");
        let unmatched = auto_type_target(&s, fg("chrome.exe", false, false));
        assert_eq!(unmatched.unwrap_err(), "no auto-type rule for chrome.exe");

        let off = Settings {
            auto_type: false,
            ..Settings::default()
        };
        let disabled = auto_type_target(&off, fg("wfica32.exe", false, false));
        assert_eq!(disabled.unwrap_err(), "auto-type is off");
    }

    #[test]
    fn hotkey_types_anywhere_but_own_or_elevated_windows() {
        let s = Settings {
            auto_type: false,
            rules: vec![Rule {
                exe: "app.exe".into(),
                enabled: true,
                press_enter: true,
                delay_ms: 99,
            }],
            ..Settings::default()
        };
        let matched = action_of(hotkey_target(&s, fg("APP.exe", false, false)));
        assert_eq!(
            matched,
            Action {
                press_enter: true,
                delay_ms: 99
            }
        );
        let other = action_of(hotkey_target(&s, fg("notepad.exe", false, false)));
        assert_eq!(other, HOTKEY_DEFAULT_ACTION);

        assert!(hotkey_target(&s, fg("app.exe", true, false)).is_err());
        assert!(hotkey_target(&s, fg("", false, true)).is_err());
        assert!(hotkey_target(&s, None).is_err());
    }

    #[test]
    fn hotkey_strings_are_parsed_by_the_plugin_parser() {
        assert!(parse_hotkey(DEFAULT_HOTKEY).unwrap().is_some());
        assert!(parse_hotkey("").unwrap().is_none());
        for bad in ["Ctrl+Alt+", "O+Ctrl", "Ctrl+Nope", "Ctrl+A+B"] {
            assert!(parse_hotkey(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn newest_clipboard_clear_wins() {
        let mut clear = ClipboardClear::default();
        let first = clear.arm(code("1111"));
        let second = clear.arm(code("2222"));
        assert!(clear.due(first).is_none());
        assert!(clear.take_if(first).is_none());
        assert_eq!(clear.due(second).unwrap().as_str(), "2222");
        // `due` leaves it pending until the clear has run.
        assert_eq!(clear.due(second).unwrap().as_str(), "2222");
        assert_eq!(clear.take_if(second).unwrap().as_str(), "2222");
        assert!(clear.due(second).is_none());
    }

    #[test]
    fn exit_takes_the_pending_clear() {
        let mut clear = ClipboardClear::default();
        assert!(clear.take().is_none());
        let seq = clear.arm(code("1111"));
        assert_eq!(clear.take().unwrap().as_str(), "1111");
        assert!(clear.due(seq).is_none());
        assert!(clear.take_if(seq).is_none());
    }

    #[test]
    fn tooltip_shows_relay_state() {
        assert_eq!(tray_tooltip(None), "OTP Forwarder – not paired");
        let retry = RelayStatus::Reconnecting {
            in_secs: 4,
            reason: "x".into(),
        };
        assert_eq!(
            tray_tooltip(Some(&retry)),
            "OTP Forwarder – reconnecting in 4 s"
        );
    }
}
