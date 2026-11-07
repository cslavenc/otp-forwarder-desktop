//! Windows implementation of [`Platform`]. The only module that uses the `windows` crate.

use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use arboard::{Clipboard, SetExtWindows};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS,
    KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyExW, SendInput,
    ToUnicodeEx, VIRTUAL_KEY, VK_CAPITAL, VK_CONTROL, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RETURN,
    VK_RWIN, VK_SHIFT, VkKeyScanExW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::{Owned, PWSTR};
use zeroize::Zeroizing;

use super::{Foreground, Platform, PlatformError};

/// How long typing waits for the user to release hotkey modifiers.
const MODIFIER_WAIT: Duration = Duration::from_secs(1);
const MODIFIER_POLL: Duration = Duration::from_millis(10);
const MODIFIERS: [VIRTUAL_KEY; 5] = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN];
/// Longer image paths just leave the exe name unknown (→ clipboard fallback).
const IMAGE_PATH_CAP: usize = 1024;
/// `VkKeyScanExW` shift-state bit for Shift.
const SHIFT_STATE: u8 = 1;

pub(super) struct WindowsPlatform {
    self_elevated: bool,
}

impl WindowsPlatform {
    pub(super) fn new() -> Self {
        // SAFETY: returns a pseudo handle for our own process that needs no closing.
        let current = unsafe { GetCurrentProcess() };
        Self {
            self_elevated: is_elevated(current).unwrap_or(false),
        }
    }
}

impl Platform for WindowsPlatform {
    fn foreground(&self) -> Option<Foreground> {
        let hwnd = foreground_window();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        // SAFETY: `pid` outlives the call; a stale `hwnd` only makes the call return 0.
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if thread == 0 {
            return None;
        }
        let process = open_process(pid);
        let exe = process.as_ref().and_then(image_name).unwrap_or_default();
        // Not inspectable counts as elevated: typing would most likely be dropped by UIPI.
        // shortcut: checks elevation, not integrity level, so UIAccess/low-IL targets aren't
        // detected; compare TokenIntegrityLevel if such a target silently drops input.
        let target_elevated = process
            .as_ref()
            .and_then(|p| is_elevated(**p))
            .unwrap_or(true);
        Some(Foreground {
            exe,
            own: pid == std::process::id(),
            elevated: target_elevated && !self.self_elevated,
            window: window_id(hwnd),
            thread,
        })
    }

    fn type_text(
        &self,
        target: &Foreground,
        text: &str,
        press_enter: bool,
        delay_ms: u32,
    ) -> Result<(), PlatformError> {
        if target.elevated {
            return Err(PlatformError::ElevatedTarget);
        }
        ensure_focus(target)?;
        wait_for_modifiers_released()?;
        // SAFETY: plain query by thread id; an exited thread yields the default layout.
        let hkl = unsafe { GetKeyboardLayout(target.thread) };
        let mut strokes: Vec<Stroke> = text
            .encode_utf16()
            .map(|unit| {
                // SAFETY: plain layout table lookup.
                let scan = unsafe { VkKeyScanExW(unit, hkl) };
                plan(unit, scan, |vk, shift, caps_lock| {
                    produces(unit, vk, shift, caps_lock, hkl)
                })
            })
            .collect();
        if press_enter {
            strokes.push(Stroke::Key {
                vk: VK_RETURN.0,
                shift: false,
            });
        }
        let total = strokes.len();
        let mut injected_shift = false;
        for (typed, stroke) in strokes.into_iter().enumerate() {
            if typed > 0 && delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms.into()));
            }
            ensure_focus(target)?;
            // Right after a shifted key our own Shift-up may not be processed yet.
            if modifier_held(!injected_shift) {
                return Err(PlatformError::ModifiersHeld);
            }
            if !send(stroke, hkl) {
                return Err(PlatformError::Partial { typed, total });
            }
            injected_shift = matches!(stroke, Stroke::Key { shift: true, .. });
        }
        Ok(())
    }

    fn copy_secret(&self, text: &str) -> Result<(), PlatformError> {
        Clipboard::new()
            .and_then(|mut clipboard| {
                clipboard
                    .set()
                    .exclude_from_history()
                    .exclude_from_cloud()
                    .text(text)
            })
            .map_err(clipboard_error)
    }

    // shortcut: get/compare/clear is not atomic, so a copy made in between can be cleared;
    // switch to GetClipboardSequenceNumber if that ever bites.
    fn clear_if_equals(&self, text: &str) -> Result<(), PlatformError> {
        let mut clipboard = Clipboard::new().map_err(clipboard_error)?;
        let current = match clipboard.get_text() {
            Ok(current) => Zeroizing::new(current),
            Err(arboard::Error::ContentNotAvailable) => return Ok(()),
            Err(e) => return Err(clipboard_error(e)),
        };
        if current.as_str() != text {
            return Ok(());
        }
        clipboard.clear().map_err(clipboard_error)
    }
}

/// One key press decided per UTF-16 unit (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stroke {
    Key { vk: u16, shift: bool },
    Unicode(u16),
}

/// Uses the target layout's key when it needs at most Shift and `produces(vk, shift,
/// caps_lock)` confirms it types exactly `unit`, else a Unicode packet. `scan` is
/// `VkKeyScanExW`'s result: low byte VK, high byte shift state, -1 if unmapped.
fn plan(unit: u16, scan: i16, produces: impl Fn(u8, bool, bool) -> bool) -> Stroke {
    let [vk, state] = scan.to_le_bytes();
    let shift = state == SHIFT_STATE;
    // The target's CapsLock state can't be read reliably from a worker thread, so the key
    // must type `unit` with CapsLock off and on (letters therefore go as Unicode).
    if state & !SHIFT_STATE == 0
        && [false, true]
            .into_iter()
            .all(|caps| produces(vk, shift, caps))
    {
        Stroke::Key {
            vk: u16::from(vk),
            shift,
        }
    } else {
        Stroke::Unicode(unit)
    }
}

/// Whether `vk` types exactly `unit` in layout `hkl` (dead keys and multi-char output don't).
fn produces(unit: u16, vk: u8, shift: bool, caps_lock: bool, hkl: HKL) -> bool {
    const DOWN: u8 = 0x80;
    const TOGGLED: u8 = 0x01;
    // ToUnicodeEx flag: don't touch the kernel keyboard state (dead-key buffer).
    const NO_STATE_CHANGE: u32 = 0x4;
    let mut keys = [0u8; 256];
    if shift {
        keys[usize::from(VK_SHIFT.0)] = DOWN;
    }
    if caps_lock {
        keys[usize::from(VK_CAPITAL.0)] = TOGGLED;
    }
    let mut out = [0u16; 4];
    let vk = VIRTUAL_KEY(vk.into());
    // SAFETY: `keys` and `out` are valid for the call; NO_STATE_CHANGE keeps it side-effect free.
    let written = unsafe {
        ToUnicodeEx(
            u32::from(vk.0),
            u32::from(scan_code(vk, hkl)),
            &keys,
            &mut out,
            NO_STATE_CHANGE,
            Some(hkl),
        )
    };
    written == 1 && out[0] == unit
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Vk(u16),
    Unicode(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyEvent {
    key: Key,
    up: bool,
}

fn key_events(stroke: Stroke) -> Vec<KeyEvent> {
    let press = |key| [KeyEvent { key, up: false }, KeyEvent { key, up: true }];
    match stroke {
        Stroke::Unicode(unit) => press(Key::Unicode(unit)).to_vec(),
        Stroke::Key { vk, shift: false } => press(Key::Vk(vk)).to_vec(),
        Stroke::Key { vk, shift: true } => {
            let [shift_down, shift_up] = press(Key::Vk(VK_LSHIFT.0));
            let [down, up] = press(Key::Vk(vk));
            vec![shift_down, down, up, shift_up]
        }
    }
}

/// Key-ups still owed when only the first `sent` events went through.
fn pending_releases(events: &[KeyEvent], sent: usize) -> Vec<KeyEvent> {
    let (done, rest) = events.split_at(sent.min(events.len()));
    rest.iter()
        .filter(|e| e.up && done.contains(&KeyEvent { up: false, ..**e }))
        .copied()
        .collect()
}

/// Sends one stroke atomically; `false` if Windows dropped any of its events.
fn send(stroke: Stroke, hkl: HKL) -> bool {
    let events = key_events(stroke);
    let sent = send_events(&events, hkl);
    if sent == events.len() {
        return true;
    }
    // Never leave a key stuck down after a partial send.
    send_events(&pending_releases(&events, sent), hkl);
    false
}

fn send_events(events: &[KeyEvent], hkl: HKL) -> usize {
    if events.is_empty() {
        return 0;
    }
    let inputs: Vec<INPUT> = events.iter().map(|e| to_input(*e, hkl)).collect();
    // SAFETY: `inputs` holds fully initialized keyboard INPUTs and `cbsize` is one INPUT's size.
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    sent as usize
}

fn to_input(event: KeyEvent, hkl: HKL) -> INPUT {
    let up = if event.up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS::default()
    };
    match event.key {
        Key::Unicode(unit) => key_input(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | up),
        Key::Vk(vk) => {
            let vk = VIRTUAL_KEY(vk);
            key_input(vk, scan_code(vk, hkl), up)
        }
    }
}

fn scan_code(vk: VIRTUAL_KEY, hkl: HKL) -> u16 {
    // SAFETY: plain layout table lookup.
    let scan = unsafe { MapVirtualKeyExW(u32::from(vk.0), MAPVK_VK_TO_VSC, Some(hkl)) };
    u16::try_from(scan).unwrap_or(0)
}

fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn ensure_focus(target: &Foreground) -> Result<(), PlatformError> {
    if window_id(foreground_window()) == target.window {
        Ok(())
    } else {
        Err(PlatformError::FocusChanged)
    }
}

/// Waits until the user lets go of the hotkey so its modifiers don't combine with our keys.
fn wait_for_modifiers_released() -> Result<(), PlatformError> {
    let deadline = Instant::now() + MODIFIER_WAIT;
    while modifier_held(true) {
        if Instant::now() >= deadline {
            return Err(PlatformError::ModifiersHeld);
        }
        sleep(MODIFIER_POLL);
    }
    Ok(())
}

fn modifier_held(include_shift: bool) -> bool {
    let mut modifiers = MODIFIERS
        .iter()
        .filter(|vk| include_shift || **vk != VK_SHIFT);
    modifiers.any(|vk| {
        // SAFETY: plain key state query; a negative value means the key is down.
        let state = unsafe { GetAsyncKeyState(i32::from(vk.0)) };
        state < 0
    })
}

fn foreground_window() -> HWND {
    // SAFETY: no preconditions; returns null when no window has focus.
    unsafe { GetForegroundWindow() }
}

fn window_id(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

fn open_process(pid: u32) -> Option<Owned<HANDLE>> {
    // SAFETY: plain query by pid.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    // SAFETY: OpenProcess succeeded, so we own this handle and `Owned` closes it once.
    Some(unsafe { Owned::new(handle) })
}

fn image_name(process: &Owned<HANDLE>) -> Option<String> {
    let mut buf = [0u16; IMAGE_PATH_CAP];
    let mut len = IMAGE_PATH_CAP as u32;
    // SAFETY: `buf` is writable for `len` UTF-16 units; `len` receives the written length.
    unsafe {
        QueryFullProcessImageNameW(
            **process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    let path = String::from_utf16_lossy(buf.get(..len as usize)?);
    Path::new(&path).file_name()?.to_str().map(str::to_owned)
}

/// `None` if the process token cannot be queried.
fn is_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: `token` receives a new handle on success.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    // SAFETY: OpenProcessToken succeeded, so we own `token` and `Owned` closes it once.
    let token = unsafe { Owned::new(token) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: the out buffer is a TOKEN_ELEVATION of exactly the given size.
    unsafe {
        GetTokenInformation(
            *token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    }
    .ok()?;
    Some(elevation.TokenIsElevated != 0)
}

fn clipboard_error(e: arboard::Error) -> PlatformError {
    PlatformError::Clipboard(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNMAPPED: i16 = -1;

    fn key(vk: u8, shift: bool) -> Stroke {
        Stroke::Key {
            vk: vk.into(),
            shift,
        }
    }

    #[test]
    fn caps_independent_keys_use_the_layout() {
        // US layout: '1' = VK 0x31, '!' = Shift + VK 0x31, both unaffected by CapsLock.
        let us = |vk: u8, shift: bool, _caps: bool| vk == 0x31 && !shift;
        assert_eq!(plan(u16::from(b'1'), 0x0031, us), key(0x31, false));
        let us = |vk: u8, shift: bool, _caps: bool| vk == 0x31 && shift;
        assert_eq!(plan(u16::from(b'!'), 0x0131, us), key(0x31, true));
    }

    #[test]
    fn caps_dependent_keys_fall_back_to_unicode() {
        // AZERTY '1' = Shift + VK 0x31, but with CapsLock on Shift gives '&'.
        let azerty = |vk: u8, shift: bool, caps: bool| vk == 0x31 && shift && !caps;
        let one = u16::from(b'1');
        assert_eq!(plan(one, 0x0131, azerty), Stroke::Unicode(one));
        // Letters flip case with CapsLock.
        let letter = |vk: u8, shift: bool, caps: bool| vk == 0x41 && shift == caps;
        let a = u16::from(b'a');
        assert_eq!(plan(a, 0x0041, letter), Stroke::Unicode(a));
        // Dead keys / multi-char output never verify.
        assert_eq!(plan(a, 0x0041, |_, _, _| false), Stroke::Unicode(a));
    }

    #[test]
    fn unmapped_or_non_shift_modifiers_fall_back_to_unicode() {
        let at = u16::from(b'@');
        let any = |_: u8, _: bool, _: bool| true;
        assert_eq!(plan(at, UNMAPPED, any), Stroke::Unicode(at));
        // German '@' = Ctrl+Alt+Q (AltGr).
        assert_eq!(plan(at, 0x0651, any), Stroke::Unicode(at));
        // Ctrl only, and Shift+Hankaku.
        assert_eq!(plan(at, 0x0241, any), Stroke::Unicode(at));
        assert_eq!(plan(at, 0x0941, any), Stroke::Unicode(at));
    }

    #[test]
    fn partial_send_releases_only_pressed_keys() {
        let events = key_events(key(0x31, true));
        let shift_up = events[3];
        let key_up = events[2];
        assert!(pending_releases(&events, 0).is_empty());
        assert_eq!(pending_releases(&events, 1), vec![shift_up]);
        assert_eq!(pending_releases(&events, 2), vec![key_up, shift_up]);
        assert_eq!(pending_releases(&events, 3), vec![shift_up]);
        assert!(pending_releases(&events, 4).is_empty());

        let unicode = key_events(Stroke::Unicode(0x41));
        assert_eq!(pending_releases(&unicode, 1), vec![unicode[1]]);
    }

    /// Run from an interactive console: `cargo test -- --ignored platform`.
    #[test]
    #[ignore = "needs an interactive desktop"]
    fn platform_foreground_and_clipboard_smoke() {
        let platform = WindowsPlatform::new();
        let fg = platform.foreground().expect("a focused window");
        assert!(!fg.exe.is_empty() || fg.elevated);

        // SAFETY: plain layout queries for this thread.
        let hkl = unsafe { GetKeyboardLayout(0) };
        let one = u16::from(b'1');
        // SAFETY: plain layout table lookup.
        let [vk, state] = unsafe { VkKeyScanExW(one, hkl) }.to_le_bytes();
        assert!(produces(one, vk, state == SHIFT_STATE, false, hkl));

        platform.copy_secret("otpfwd-smoke-1234").unwrap();
        platform.clear_if_equals("something-else").unwrap();
        assert_eq!(
            Clipboard::new().unwrap().get_text().unwrap(),
            "otpfwd-smoke-1234"
        );
        platform.clear_if_equals("otpfwd-smoke-1234").unwrap();
        assert!(Clipboard::new().unwrap().get_text().is_err());
    }
}
