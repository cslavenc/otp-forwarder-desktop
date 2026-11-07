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
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC,
    MapVirtualKeyExW, SendInput, VIRTUAL_KEY, VK_CAPITAL, VK_CONTROL, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_RETURN, VK_RWIN, VK_SHIFT, VkKeyScanExW,
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
        // SAFETY: plain key state query; the low bit is the toggle state.
        let caps_lock = unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) } & 1 != 0;
        let mut strokes: Vec<Stroke> = text
            .encode_utf16()
            .map(|unit| {
                // SAFETY: plain layout table lookup.
                let scan = unsafe { VkKeyScanExW(unit, hkl) };
                plan(unit, scan, caps_lock)
            })
            .collect();
        if press_enter {
            strokes.push(Stroke::Key {
                vk: VK_RETURN.0,
                shift: false,
            });
        }
        let total = strokes.len();
        for (typed, stroke) in strokes.into_iter().enumerate() {
            if typed > 0 && delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms.into()));
            }
            ensure_focus(target)?;
            if !send(stroke, hkl) {
                return Err(PlatformError::Partial { typed, total });
            }
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

/// Uses the target layout's key when it needs at most Shift, else a Unicode packet.
/// `scan` is `VkKeyScanExW`'s result: low byte VK, high byte shift state, -1 if unmapped.
fn plan(unit: u16, scan: i16, caps_lock: bool) -> Stroke {
    let [vk, state] = scan.to_le_bytes();
    if state & !SHIFT_STATE != 0 {
        return Stroke::Unicode(unit);
    }
    // CapsLock inverts Shift for letter keys.
    let invert = caps_lock && vk.is_ascii_uppercase();
    Stroke::Key {
        vk: u16::from(vk),
        shift: (state == SHIFT_STATE) != invert,
    }
}

/// Sends one stroke atomically; `false` if Windows dropped any of its events.
fn send(stroke: Stroke, hkl: HKL) -> bool {
    let inputs = match stroke {
        Stroke::Unicode(unit) => vec![
            key_input(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE),
            key_input(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        ],
        Stroke::Key { vk, shift } => {
            let vk = VIRTUAL_KEY(vk);
            let mut inputs = Vec::with_capacity(4);
            if shift {
                inputs.push(vk_input(VK_LSHIFT, hkl, KEYBD_EVENT_FLAGS::default()));
            }
            inputs.push(vk_input(vk, hkl, KEYBD_EVENT_FLAGS::default()));
            inputs.push(vk_input(vk, hkl, KEYEVENTF_KEYUP));
            if shift {
                inputs.push(vk_input(VK_LSHIFT, hkl, KEYEVENTF_KEYUP));
            }
            inputs
        }
    };
    let sent = send_inputs(&inputs);
    if sent > 0 && sent < inputs.len() && matches!(stroke, Stroke::Key { shift: true, .. }) {
        // Never leave Shift stuck down after a partial send.
        send_inputs(&[vk_input(VK_LSHIFT, hkl, KEYEVENTF_KEYUP)]);
    }
    sent == inputs.len()
}

fn send_inputs(inputs: &[INPUT]) -> usize {
    // SAFETY: `inputs` holds fully initialized keyboard INPUTs and `cbsize` is one INPUT's size.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    sent as usize
}

fn vk_input(vk: VIRTUAL_KEY, hkl: HKL, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    // SAFETY: plain layout table lookup.
    let scan = unsafe { MapVirtualKeyExW(u32::from(vk.0), MAPVK_VK_TO_VSC, Some(hkl)) };
    key_input(vk, u16::try_from(scan).unwrap_or(0), flags)
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
    while modifier_held() {
        if Instant::now() >= deadline {
            return Err(PlatformError::ModifiersHeld);
        }
        sleep(MODIFIER_POLL);
    }
    Ok(())
}

fn modifier_held() -> bool {
    MODIFIERS.iter().any(|vk| {
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
    fn plain_and_shifted_keys_use_the_layout() {
        // US layout: '1' = VK 0x31, 'a' = VK 0x41, 'A' = Shift + VK 0x41.
        assert_eq!(plan(u16::from(b'1'), 0x0031, false), key(0x31, false));
        assert_eq!(plan(u16::from(b'a'), 0x0041, false), key(0x41, false));
        assert_eq!(plan(u16::from(b'A'), 0x0141, false), key(0x41, true));
    }

    #[test]
    fn caps_lock_inverts_shift_for_letters_only() {
        assert_eq!(plan(u16::from(b'a'), 0x0041, true), key(0x41, true));
        assert_eq!(plan(u16::from(b'A'), 0x0141, true), key(0x41, false));
        assert_eq!(plan(u16::from(b'1'), 0x0031, true), key(0x31, false));
        // '!' = Shift + '1' stays shifted.
        assert_eq!(plan(u16::from(b'!'), 0x0131, true), key(0x31, true));
    }

    #[test]
    fn unmapped_or_non_shift_modifiers_fall_back_to_unicode() {
        let at = u16::from(b'@');
        assert_eq!(plan(at, UNMAPPED, false), Stroke::Unicode(at));
        // German '@' = Ctrl+Alt+Q (AltGr).
        assert_eq!(plan(at, 0x0651, false), Stroke::Unicode(at));
        // Ctrl only, and Shift+Hankaku.
        assert_eq!(plan(at, 0x0241, false), Stroke::Unicode(at));
        assert_eq!(plan(at, 0x0941, false), Stroke::Unicode(at));
    }

    /// Run from an interactive console: `cargo test -- --ignored platform`.
    #[test]
    #[ignore = "needs an interactive desktop"]
    fn platform_foreground_and_clipboard_smoke() {
        let platform = WindowsPlatform::new();
        let fg = platform.foreground().expect("a focused window");
        assert!(!fg.exe.is_empty() || fg.elevated);

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
