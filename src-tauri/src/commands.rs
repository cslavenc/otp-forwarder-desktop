//! Thin IPC layer: translates between the webview and the pipeline. Errors become strings.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::pipeline::{self, AppState, PairingState, RecentView};
use crate::relay::RelayStatus;
use crate::settings::Settings;

#[derive(Serialize)]
pub struct Status {
    paired: bool,
    /// `None` while unpaired.
    relay: Option<RelayStatus>,
}

/// Never contains the key or topic.
#[derive(Serialize)]
pub struct PairingInfo {
    paired: bool,
    relay: Option<String>,
    /// Why the stored pairing could not be loaded.
    error: Option<String>,
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> Status {
    let inner = state.lock();
    Status {
        paired: matches!(inner.pairing, PairingState::Paired(_)),
        relay: inner.relay.clone(),
    }
}

#[tauri::command]
pub fn get_pairing_info(state: State<'_, AppState>) -> PairingInfo {
    match &state.lock().pairing {
        PairingState::Paired(p) => PairingInfo {
            paired: true,
            relay: Some(p.relay.as_str().trim_end_matches('/').to_owned()),
            error: None,
        },
        PairingState::Unpaired { detail } => PairingInfo {
            paired: false,
            relay: None,
            error: Some(detail.clone()),
        },
    }
}

/// SVG QR code of the pairing JSON (contains the key).
#[tauri::command]
pub fn pairing_qr(state: State<'_, AppState>) -> Result<String, String> {
    match &state.lock().pairing {
        PairingState::Paired(p) => p.qr_svg().map_err(|e| e.to_string()),
        PairingState::Unpaired { .. } => Err("not paired".into()),
    }
}

#[tauri::command]
pub fn repair(app: AppHandle) -> Result<(), String> {
    pipeline::repair(&app)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.lock().settings.clone()
}

/// Validates, swaps the hotkey, then saves; nothing changes on error. A hotkey that cannot be
/// parsed or registered does not block the save: the stored hotkey is kept and `Some(warning)`
/// is returned. Sync commands run on the main thread, so the hotkey plugin's main-thread calls
/// run inline while the state is locked.
#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: Settings,
) -> Result<Option<String>, String> {
    settings.validate().map_err(|e| e.to_string())?;
    let mut inner = state.lock();
    if !inner.settings_writable {
        return Err("settings file unreadable; restart the app".into());
    }
    let old = inner.settings.hotkey.clone();
    // An invalid stored hotkey was never registered.
    let old_key = pipeline::parse_hotkey(&old).unwrap_or(None);
    let warning = match pipeline::parse_hotkey(&settings.hotkey)
        .and_then(|new| pipeline::swap_hotkey(&app, old_key, new))
    {
        Ok(()) => None,
        Err(e) => {
            let tried = std::mem::replace(&mut settings.hotkey, old);
            Some(format!("{tried} is not active: {e}"))
        }
    };
    if let Err(e) = settings.save(&state.settings_path) {
        let new_key = pipeline::parse_hotkey(&settings.hotkey).ok().flatten();
        if new_key != old_key {
            let _ = pipeline::swap_hotkey(&app, new_key, old_key);
        }
        return Err(e.to_string());
    }
    inner.settings = settings;
    Ok(warning)
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let autolaunch = app.autolaunch();
    let result = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_recent(state: State<'_, AppState>) -> Vec<RecentView> {
    state.lock().recent.views(pipeline::now_ms())
}

#[tauri::command]
pub fn reveal_code(state: State<'_, AppState>, id: String) -> Result<String, String> {
    state
        .lock()
        .recent
        .code(&id, pipeline::now_ms())
        .map(|code| code.to_string())
        .ok_or_else(|| "code expired".into())
}

#[tauri::command]
pub async fn copy_code(app: AppHandle, id: String) -> Result<(), String> {
    let code = app
        .state::<AppState>()
        .lock()
        .recent
        .code(&id, pipeline::now_ms());
    pipeline::copy_secret(&app, code.ok_or("code expired")?).await
}

/// Waits 5 s, then publishes a test code to our own pairing; resolves once it is published.
#[tauri::command]
pub async fn send_test_code(app: AppHandle) -> Result<(), String> {
    pipeline::send_test_code(&app).await
}
