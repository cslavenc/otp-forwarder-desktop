//! OS seam: foreground app detection, keystroke typing and secret clipboard.
//! Only `platform::windows` touches the `windows` crate.

#[cfg(windows)]
mod windows;

/// The window that currently has keyboard focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    /// Image file name (e.g. `wfica32.exe`); empty if the process could not be inspected.
    pub exe: String,
    /// The window belongs to this app.
    pub own: bool,
    /// Typing would be blocked by UIPI: the target is elevated (or not inspectable) while we
    /// are not.
    pub elevated: bool,
    /// Opaque window handle, compared to detect focus changes.
    pub(crate) window: u64,
    pub(crate) thread: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("focus moved to another window")]
    FocusChanged,
    #[error("target window runs elevated")]
    ElevatedTarget,
    #[error("modifier keys are still held")]
    ModifiersHeld,
    #[error("typing was interrupted after {typed} of {total} keys")]
    Partial { typed: usize, total: usize },
    #[error("clipboard: {0}")]
    Clipboard(String),
}

pub trait Platform: Send + Sync {
    fn foreground(&self) -> Option<Foreground>;
    /// Types `text` into `target`, `delay_ms` between keys, re-checking focus before each key.
    fn type_text(
        &self,
        target: &Foreground,
        text: &str,
        press_enter: bool,
        delay_ms: u32,
    ) -> Result<(), PlatformError>;
    /// Copies `text`, excluded from clipboard history and cloud sync.
    fn copy_secret(&self, text: &str) -> Result<(), PlatformError>;
    /// Clears the clipboard only if it still holds exactly `text`.
    fn clear_if_equals(&self, text: &str) -> Result<(), PlatformError>;
}

#[cfg(windows)]
pub fn native() -> Box<dyn Platform> {
    Box::new(windows::WindowsPlatform::new())
}
