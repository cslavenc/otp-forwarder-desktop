//! Publishes an encrypted OTP to the paired relay topic, for testing without the phone.
//!
//! cargo run --manifest-path src-tauri/Cargo.toml --example otpfwd-send -- --code 123456
//!     [--pairing <file>] [--sender <S>] [--id <ID>] [--age-secs <N>] [--delay-secs <N>]
//!
//! Without `--pairing` the pairing is read from the OS keyring. Prints only the message id.

use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use otp_forwarder_desktop_lib::pairing::Pairing;
use otp_forwarder_desktop_lib::{crypto, otp, relay};
use zeroize::Zeroizing;

const USAGE: &str = "usage: otpfwd-send --code <CODE> [--pairing <file>] [--sender <S>] [--id <ID>] \
                     [--age-secs <N>] [--delay-secs <N>]";

#[derive(Default)]
struct Args {
    code: Zeroizing<String>,
    pairing: Option<String>,
    sender: String,
    id: Option<String>,
    /// Backdates `ts`; negative values date it into the future.
    age_secs: i64,
    delay_secs: u64,
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match send(args) {
        Ok(id) => {
            println!("{id}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args::default();
    let mut code = None;
    while let Some(flag) = it.next() {
        // A stray value could be the code, so it is never echoed.
        if !flag.starts_with("--") {
            return Err("unexpected argument (not shown)".into());
        }
        let value = it.next().ok_or(format!("missing value for {flag}"));
        match flag.as_str() {
            "--code" => code = Some(Zeroizing::new(value?)),
            "--pairing" => args.pairing = Some(value?),
            "--sender" => args.sender = value?,
            "--id" => args.id = Some(value?),
            "--age-secs" => args.age_secs = number(&flag, &value?)?,
            "--delay-secs" => args.delay_secs = number(&flag, &value?)?,
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    args.code = code.ok_or("--code is required")?;
    if !otp::is_valid_code(&args.code) {
        return Err("--code must be 4-10 ASCII letters or digits".into());
    }
    Ok(args)
}

fn number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} needs a whole number"))
}

fn send(args: Args) -> Result<String, String> {
    let pairing = load_pairing(args.pairing.as_deref())?;
    let id = match args.id {
        Some(id) => id,
        None => otp::new_id().map_err(|e| e.to_string())?,
    };
    sleep(Duration::from_secs(args.delay_secs));
    let ts = now_ms().saturating_sub(args.age_secs.saturating_mul(1000));
    let json = otp::encode_plaintext(&id, &args.code, &args.sender, ts);
    let body = crypto::encrypt(&pairing.key, &pairing.topic, json.as_bytes())
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::block_on(async {
        let http = relay::http_client()?;
        relay::publish(&http, &pairing.relay, &pairing.topic, &body).await
    })
    .map_err(|e| e.to_string())?;
    Ok(id)
}

fn load_pairing(file: Option<&str>) -> Result<Pairing, String> {
    let Some(path) = file else {
        return Pairing::load()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no pairing in the keyring; pass --pairing <file>".into());
    };
    let json = Zeroizing::new(
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?,
    );
    Pairing::from_json(json.trim()).map_err(|e| e.to_string())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|a| (*a).to_owned()))
    }

    fn error(args: &[&str]) -> String {
        match parse(args) {
            Err(e) => e,
            Ok(_) => panic!("arguments were unexpectedly accepted"),
        }
    }

    #[test]
    fn parses_all_flags() {
        let args = parse(&[
            "--code",
            "AbC123",
            "--pairing",
            "p.json",
            "--sender",
            "Bank",
            "--id",
            "x",
            "--age-secs",
            "-30",
            "--delay-secs",
            "2",
        ])
        .unwrap();
        assert_eq!(args.code.as_str(), "AbC123");
        assert_eq!(args.pairing.as_deref(), Some("p.json"));
        assert_eq!(args.sender, "Bank");
        assert_eq!(args.id.as_deref(), Some("x"));
        assert_eq!(args.age_secs, -30);
        assert_eq!(args.delay_secs, 2);
    }

    #[test]
    fn rejects_missing_or_invalid_code() {
        assert!(error(&["--sender", "Bank"]).contains("--code is required"));
        let e = error(&["--code", "12-34"]);
        assert!(e.contains("--code must be") && !e.contains("12-34"), "{e}");
    }

    #[test]
    fn never_echoes_positional_arguments() {
        for args in [&["123456"][..], &["--code", "123456", "654321"]] {
            let e = error(args);
            assert!(!e.contains("123456") && !e.contains("654321"), "{e}");
        }
        // A flag missing its value must not swallow-and-echo the next one either.
        let e = error(&["--sender", "--code", "123456"]);
        assert!(!e.contains("123456"), "{e}");
    }

    #[test]
    fn rejects_bad_flags_and_values() {
        assert!(error(&["--code", "1234", "--sender"]).contains("missing value for --sender"));
        assert!(error(&["--code", "1234", "--bogus", "1"]).contains("unknown argument --bogus"));
        assert!(error(&["--code", "1234", "--age-secs", "ten"]).contains("whole number"));
        assert!(error(&["--code", "1234", "--delay-secs", "-1"]).contains("whole number"));
    }
}
