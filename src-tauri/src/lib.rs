pub mod commands;
pub mod crypto;
pub mod otp;
pub mod pairing;
pub mod pipeline;
pub mod platform;
pub mod relay;
pub mod rules;
pub mod settings;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::ShortcutState;
use tokio::sync::mpsc;

use pairing::{DEFAULT_RELAY, Pairing};
use pipeline::{AppState, PairingState};
use settings::Settings;

/// Passed by autostart; the window then stays hidden if paired.
const MINIMIZED_ARG: &str = "--minimized";
const MAIN_WINDOW: &str = "main";
const SETTINGS_FILE: &str = "settings.json";
const RELAY_QUEUE: usize = 32;
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // First, so a second launch exits before initializing anything else.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        pipeline::on_hotkey(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![MINIMIZED_ARG]),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_pairing_info,
            commands::pairing_qr,
            commands::repair,
            commands::get_settings,
            commands::set_settings,
            commands::get_autostart,
            commands::set_autostart,
            commands::list_recent,
            commands::reveal_code,
            commands::copy_code,
            commands::send_test_code,
            commands::detect_app,
        ])
        .setup(|app| setup(app.handle()))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                // Lets the UI drop the pairing QR (it holds the key) while hidden in the tray.
                let _ = window.emit("hidden", ());
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                pipeline::clear_clipboard_now(app);
            }
        });
}

fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let settings_path = app.path().app_config_dir()?.join(SETTINGS_FILE);
    let (settings, settings_error) = match Settings::load(&settings_path) {
        Ok(settings) => (settings, None),
        Err(e) => {
            pipeline::debug_log(&format!("settings unreadable, using defaults: {e}"));
            let error = format!(
                "Settings could not be read; changes won't be saved until you restart: {e}"
            );
            (Settings::default(), Some(error))
        }
    };
    let hotkey = settings.hotkey.clone();
    let notify = settings.notifications;
    let pairing = match Pairing::entry() {
        Ok(entry) => load_or_create_pairing(&entry),
        Err(e) => PairingState::Unpaired {
            detail: e.to_string(),
        },
    };
    let paired = matches!(pairing, PairingState::Paired(_));

    let (relay_tx, relay_rx) = mpsc::channel(RELAY_QUEUE);
    app.manage(AppState::new(
        platform::native(),
        relay::http_client()?,
        settings_path,
        relay_tx,
        pairing,
        settings,
        settings_error.clone(),
    ));
    if let Some(error) = &settings_error {
        pipeline::toast(app, notify, error);
    }
    // Tray first, so the relay's first status reaches its tooltip.
    build_tray(app)?;
    tauri::async_runtime::spawn(pipeline::run(app.clone(), relay_rx));
    pipeline::restart_relay(app);

    let registered = pipeline::parse_hotkey(&hotkey)
        .and_then(|shortcut| pipeline::swap_hotkey(app, None, shortcut));
    if let Err(e) = registered {
        pipeline::toast(app, notify, &format!("Hotkey not active: {e}"));
    }

    let minimized = std::env::args().any(|arg| arg == MINIMIZED_ARG);
    if !(minimized && paired) {
        show_main_window(app);
    }
    Ok(())
}

/// Only a missing pairing is generated (03 #13); an unreadable one is kept for the user to
/// replace explicitly via re-pair.
fn load_or_create_pairing(entry: &keyring::Entry) -> PairingState {
    let pairing = match Pairing::load_from(entry) {
        Ok(Some(pairing)) => Ok(pairing),
        Ok(None) => Pairing::generate(DEFAULT_RELAY).and_then(|p| p.save_to(entry).map(|()| p)),
        Err(e) => Err(e),
    };
    match pairing {
        Ok(pairing) => PairingState::Paired(pairing),
        Err(e) => PairingState::Unpaired {
            detail: e.to_string(),
        },
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_SHOW, "Show", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let tooltip = pipeline::tray_tooltip(app.state::<AppState>().lock().relay.as_ref());
    let mut tray = TrayIconBuilder::with_id(pipeline::TRAY_ID)
        .tooltip(tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_main_window(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    unhide_main_window(app);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.set_focus();
    }
}

/// Shows the window without stealing focus from the app the user just switched to.
pub(crate) fn unhide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
    }
}

#[cfg(test)]
mod tests {
    use keyring::mock::MockCredential;

    use super::*;

    fn mock_entry() -> keyring::Entry {
        keyring::Entry::new_with_credential(Box::new(MockCredential::default()))
    }

    /// Canary for tauri#13419: this test binary links Tauri, so it only starts if build.rs
    /// embedded the Common-Controls v6 manifest.
    #[test]
    fn test_binary_with_tauri_starts() {
        let _linked: fn() = super::run;
    }

    #[test]
    fn missing_pairing_is_generated_and_saved() {
        let entry = mock_entry();
        let PairingState::Paired(pairing) = load_or_create_pairing(&entry) else {
            panic!("expected a new pairing");
        };
        let saved = entry.get_password().unwrap();
        assert!(saved.contains(&pairing.topic));
    }

    #[test]
    fn keyring_error_leaves_the_pairing_untouched() {
        let entry = mock_entry();
        let stored = Pairing::generate(DEFAULT_RELAY).unwrap();
        stored.save_to(&entry).unwrap();
        let before = entry.get_password().unwrap();
        let mock: &MockCredential = entry.get_credential().downcast_ref().unwrap();
        mock.set_error(keyring::Error::PlatformFailure("locked".into()));

        assert!(matches!(
            load_or_create_pairing(&entry),
            PairingState::Unpaired { .. }
        ));
        assert_eq!(entry.get_password().unwrap(), before);
    }

    #[test]
    fn corrupt_pairing_is_not_overwritten() {
        let entry = mock_entry();
        entry.set_password("{corrupt").unwrap();
        assert!(matches!(
            load_or_create_pairing(&entry),
            PairingState::Unpaired { .. }
        ));
        assert_eq!(entry.get_password().unwrap(), "{corrupt");
    }
}
