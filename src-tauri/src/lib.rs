pub mod crypto;
pub mod otp;
pub mod pairing;
pub mod platform;
pub mod relay;
pub mod rules;
pub mod settings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    /// Canary for tauri#13419: this test binary links Tauri, so it only starts if build.rs
    /// embedded the Common-Controls v6 manifest.
    #[test]
    fn test_binary_with_tauri_starts() {
        let _linked: fn() = super::run;
    }
}
