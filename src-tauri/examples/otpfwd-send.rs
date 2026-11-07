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
use serde::Serialize;
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

#[derive(Serialize)]
struct Plaintext<'a> {
    v: u8,
    id: &'a str,
    code: &'a str,
    sender: &'a str,
    ts: i64,
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
    let plaintext = Plaintext {
        v: otp::VERSION,
        id: &id,
        code: &args.code,
        sender: &args.sender,
        ts: now_ms().saturating_sub(args.age_secs.saturating_mul(1000)),
    };
    let json = Zeroizing::new(serde_json::to_string(&plaintext).map_err(|e| e.to_string())?);
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
