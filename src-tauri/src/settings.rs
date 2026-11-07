//! User settings, persisted as JSON in the app config dir.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_RULES: usize = 50;
pub const MAX_EXE_CHARS: usize = 64;
/// Delay between typed characters.
pub const DEFAULT_DELAY_MS: u32 = 15;
pub const MAX_DELAY_MS: u32 = 500;
pub const DEFAULT_CLIPBOARD_CLEAR_SECS: u32 = 30;
pub const MAX_CLIPBOARD_CLEAR_SECS: u32 = 600;
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Shift+O";
const DEFAULT_RULE_EXES: [&str; 3] = ["wfica32.exe", "CDViewer.exe", "SelfService.exe"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub auto_type: bool,
    pub rules: Vec<Rule>,
    /// 0 = never clear.
    pub clipboard_clear_secs: u32,
    pub notifications: bool,
    /// Global shortcut that types the last code; "" = off. Parsed when registered.
    pub hotkey: String,
}

/// Auto-type rule for one foreground process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// Bare executable file name, matched case-insensitively.
    pub exe: String,
    pub enabled: bool,
    pub press_enter: bool,
    pub delay_ms: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("settings JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid settings: {0}")]
    Invalid(String),
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_type: true,
            rules: DEFAULT_RULE_EXES.into_iter().map(Rule::for_exe).collect(),
            clipboard_clear_secs: DEFAULT_CLIPBOARD_CLEAR_SECS,
            notifications: true,
            hotkey: DEFAULT_HOTKEY.to_owned(),
        }
    }
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            exe: String::new(),
            enabled: true,
            press_enter: false,
            delay_ms: DEFAULT_DELAY_MS,
        }
    }
}

impl Rule {
    fn for_exe(exe: &str) -> Self {
        Self {
            exe: exe.to_owned(),
            ..Self::default()
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.rules.len() > MAX_RULES {
            return Err(SettingsError::Invalid(format!(
                "at most {MAX_RULES} rules are allowed"
            )));
        }
        for (i, rule) in self.rules.iter().enumerate() {
            if !is_bare_exe_name(&rule.exe) {
                return Err(SettingsError::Invalid(format!(
                    "rule {}: exe must be a file name ending in .exe (max {MAX_EXE_CHARS} chars)",
                    i + 1
                )));
            }
            if rule.delay_ms > MAX_DELAY_MS {
                return Err(SettingsError::Invalid(format!(
                    "rule {}: delay must be 0-{MAX_DELAY_MS} ms",
                    i + 1
                )));
            }
        }
        if self.clipboard_clear_secs > MAX_CLIPBOARD_CLEAR_SECS {
            return Err(SettingsError::Invalid(format!(
                "clipboard clear must be 0-{MAX_CLIPBOARD_CLEAR_SECS} s"
            )));
        }
        Ok(())
    }

    /// Missing file → defaults. Invalid content → moved to `<path>.bak`, defaults.
    /// Read errors (e.g. file locked) are returned so the caller does not later overwrite the
    /// user's file with defaults.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        Ok(match Self::parse(&bytes) {
            Ok(settings) => settings,
            Err(e) => {
                debug_warn(&format!("settings corrupt, moving to .bak: {e}"));
                if let Err(e) = fs::rename(path, with_suffix(path, ".bak")) {
                    debug_warn(&format!("cannot back up corrupt settings: {e}"));
                }
                Self::default()
            }
        })
    }

    /// Validates, then writes atomically (temp file + rename). Callers serialize saves (settings
    /// mutex) because the temp file name is fixed.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        self.validate()?;
        let json = serde_json::to_vec_pretty(self)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = with_suffix(path, ".tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&json)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    fn parse(bytes: &[u8]) -> Result<Self, SettingsError> {
        let settings: Self = serde_json::from_slice(bytes)?;
        settings.validate()?;
        Ok(settings)
    }
}

fn is_bare_exe_name(exe: &str) -> bool {
    let chars = exe.chars().count();
    chars > ".exe".len()
        && chars <= MAX_EXE_CHARS
        && exe.to_ascii_lowercase().ends_with(".exe")
        && !exe
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
}

/// `settings.json` + `.bak` → `settings.json.bak`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

fn debug_warn(message: &str) {
    if cfg!(debug_assertions) {
        eprintln!("[settings] {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh, empty per-test directory under the system temp dir.
    fn temp_dir(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("otpfwd-settings-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn invalid(settings: &Settings) -> bool {
        matches!(settings.validate(), Err(SettingsError::Invalid(_)))
    }

    #[test]
    fn defaults_match_decisions() {
        let s = Settings::default();
        assert!(s.auto_type);
        assert!(s.notifications);
        assert_eq!(s.clipboard_clear_secs, 30);
        assert_eq!(s.hotkey, "Ctrl+Alt+Shift+O");
        let exes: Vec<&str> = s.rules.iter().map(|r| r.exe.as_str()).collect();
        assert_eq!(exes, ["wfica32.exe", "CDViewer.exe", "SelfService.exe"]);
        for rule in &s.rules {
            assert!(rule.enabled);
            assert!(!rule.press_enter);
            assert_eq!(rule.delay_ms, 15);
        }
        assert!(s.validate().is_ok());
    }

    #[test]
    fn delay_and_clipboard_boundaries() {
        let mut s = Settings {
            clipboard_clear_secs: 0,
            ..Settings::default()
        };
        s.rules[0].delay_ms = 0;
        assert!(s.validate().is_ok());
        s.rules[0].delay_ms = MAX_DELAY_MS;
        s.clipboard_clear_secs = MAX_CLIPBOARD_CLEAR_SECS;
        assert!(s.validate().is_ok());
        s.rules[0].delay_ms = MAX_DELAY_MS + 1;
        assert!(invalid(&s));
        s.rules[0].delay_ms = 0;
        s.clipboard_clear_secs = MAX_CLIPBOARD_CLEAR_SECS + 1;
        assert!(invalid(&s));
    }

    #[test]
    fn rule_count_boundary() {
        let mut s = Settings {
            rules: vec![Rule::for_exe("a.exe"); MAX_RULES],
            ..Settings::default()
        };
        assert!(s.validate().is_ok());
        s.rules.push(Rule::for_exe("a.exe"));
        assert!(invalid(&s));
    }

    #[test]
    fn exe_name_rules() {
        let max = format!("{}.exe", "a".repeat(MAX_EXE_CHARS - 4));
        for ok in [
            "wfica32.exe",
            "WFICA32.EXE",
            "my app.exe",
            "ä.exe",
            max.as_str(),
        ] {
            assert!(is_bare_exe_name(ok), "{ok}");
        }
        let too_long = format!("a{max}");
        for bad in [
            "",
            ".exe",
            "wfica32",
            "x.com",
            "C:\\x.exe",
            "dir/x.exe",
            "dir\\x.exe",
            "c:x.exe",
            "x\n.exe",
            too_long.as_str(),
        ] {
            assert!(!is_bare_exe_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn load_missing_file_gives_defaults() {
        let dir = temp_dir("missing");
        assert_eq!(
            Settings::load(&dir.join("settings.json")).unwrap(),
            Settings::default()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_returns_read_errors_instead_of_defaults() {
        let dir = temp_dir("unreadable");
        // Reading a directory fails with an error other than NotFound.
        assert!(matches!(Settings::load(&dir), Err(SettingsError::Io(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("round-trip");
        let path = dir.join("nested").join("settings.json");
        let mut s = Settings {
            auto_type: false,
            hotkey: String::new(),
            ..Settings::default()
        };
        s.rules[1].press_enter = true;
        s.rules.push(Rule::for_exe("chrome.exe"));

        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        assert!(!with_suffix(&path, ".tmp").exists());

        s.notifications = false;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_default_and_unknown_fields_are_ignored() {
        let dir = temp_dir("partial");
        let path = dir.join("settings.json");
        fs::write(
            &path,
            r#"{"auto_type":false,"rules":[{"exe":"x.exe"}],"future":1}"#,
        )
        .unwrap();
        let s = Settings::load(&path).unwrap();
        assert!(!s.auto_type);
        assert_eq!(s.rules, vec![Rule::for_exe("x.exe")]);
        assert_eq!(s.clipboard_clear_secs, DEFAULT_CLIPBOARD_CLEAR_SECS);
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_or_invalid_file_is_backed_up() {
        let cases: [&[u8]; 4] = [
            b"{not json",
            b"42",
            b"\xff\xfe garbage",
            br#"{"rules":[{"exe":"x.exe","delay_ms":501}]}"#,
        ];
        for (i, content) in cases.into_iter().enumerate() {
            let dir = temp_dir(&format!("corrupt-{i}"));
            let path = dir.join("settings.json");
            fs::write(&path, content).unwrap();
            assert_eq!(Settings::load(&path).unwrap(), Settings::default());
            assert!(!path.exists());
            assert_eq!(fs::read(with_suffix(&path, ".bak")).unwrap(), content);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn save_rejects_invalid_settings_without_writing() {
        let dir = temp_dir("save-invalid");
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.rules[0].exe = "C:\\evil.exe".into();
        assert!(matches!(s.save(&path), Err(SettingsError::Invalid(_))));
        assert!(!path.exists());
        assert!(!with_suffix(&path, ".tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
