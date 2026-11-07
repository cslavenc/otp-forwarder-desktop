//! Decides whether a code is auto-typed into the current foreground app.

use crate::settings::Settings;

/// How to type into a matched app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub press_enter: bool,
    /// Delay between typed characters.
    pub delay_ms: u32,
}

/// `Some` → auto-type; `None` → clipboard fallback. Matches the first enabled rule whose exe
/// equals the foreground exe name, case-insensitively.
pub fn decide(settings: &Settings, foreground_exe: Option<&str>) -> Option<Action> {
    if !settings.auto_type {
        return None;
    }
    let exe = foreground_exe?.to_lowercase();
    settings
        .rules
        .iter()
        .find(|rule| rule.enabled && rule.exe.to_lowercase() == exe)
        .map(|rule| Action {
            press_enter: rule.press_enter,
            delay_ms: rule.delay_ms,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Rule;

    fn rule(exe: &str, enabled: bool, press_enter: bool, delay_ms: u32) -> Rule {
        Rule {
            exe: exe.to_owned(),
            enabled,
            press_enter,
            delay_ms,
        }
    }

    fn with_rules(rules: Vec<Rule>) -> Settings {
        Settings {
            rules,
            ..Settings::default()
        }
    }

    #[test]
    fn default_rules_match_case_insensitively() {
        let s = Settings::default();
        let expected = Some(Action {
            press_enter: false,
            delay_ms: 15,
        });
        for exe in [
            "wfica32.exe",
            "WFICA32.EXE",
            "cdviewer.exe",
            "SelfService.EXE",
        ] {
            assert_eq!(decide(&s, Some(exe)), expected, "{exe}");
        }
    }

    #[test]
    fn unmatched_or_unknown_foreground_falls_back() {
        let s = Settings::default();
        assert_eq!(decide(&s, Some("chrome.exe")), None);
        assert_eq!(decide(&s, Some("wfica32")), None);
        assert_eq!(decide(&s, Some("")), None);
        assert_eq!(decide(&s, None), None);
    }

    #[test]
    fn auto_type_off_never_types() {
        let s = Settings {
            auto_type: false,
            ..Settings::default()
        };
        assert_eq!(decide(&s, Some("wfica32.exe")), None);
    }

    #[test]
    fn disabled_rule_is_skipped_and_rule_values_are_used() {
        let s = with_rules(vec![
            rule("app.exe", false, false, 1),
            rule("APP.exe", true, true, 200),
        ]);
        assert_eq!(
            decide(&s, Some("app.exe")),
            Some(Action {
                press_enter: true,
                delay_ms: 200,
            })
        );
        let s = with_rules(vec![rule("app.exe", false, true, 0)]);
        assert_eq!(decide(&s, Some("app.exe")), None);
    }

    #[test]
    fn non_ascii_names_match_case_insensitively() {
        let s = with_rules(vec![rule("Ärzte.exe", true, false, 0)]);
        assert!(decide(&s, Some("ärzte.EXE")).is_some());
    }
}
