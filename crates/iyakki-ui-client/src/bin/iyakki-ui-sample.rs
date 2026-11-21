//! SAMPLE UI program for the contract: a text screen of the machine's settings that uses only `iyakki-ui-client`.
//!
//! ```text
//! iyakki-ui-sample ADDRESS [--set KEY VALUE]
//! ```
//!
//! It says hello, negotiates the `settings` domain (a machine without it gets a plain note, not a crash), prints every setting with its
//! value, and with `--set` changes one and prints what the machine answered, refusals included. Exit status: 0 ok, 1 a refusal or an
//! error, 2 bad arguments.

use std::time::Duration;

use iyakki_ui_client::client::{Connection, RemoteSettings};
use iyakki_ui_client::{CallContext, SettingKind, SettingValue, Settings};

fn show(v: &SettingValue) -> String {
    match v {
        SettingValue::Bool(b) => if *b { "on".into() } else { "off".into() },
        SettingValue::Int(n) => n.to_string(),
        SettingValue::Text(t) => t.clone(),
    }
}

fn parse(kind: &SettingKind, text: &str) -> Option<SettingValue> {
    match kind {
        SettingKind::Bool => match text {
            "on" | "true" => Some(SettingValue::Bool(true)),
            "off" | "false" => Some(SettingValue::Bool(false)),
            _ => None,
        },
        SettingKind::Int { .. } => text.parse().ok().map(SettingValue::Int),
        SettingKind::Text { .. } | SettingKind::Choice { .. } => Some(SettingValue::Text(text.to_string())),
        // a kind a newer machine added: this sample cannot write it
        SettingKind::Unknown => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(addr) = args.first() else {
        eprintln!("usage: iyakki-ui-sample ADDRESS [--set KEY VALUE]");
        std::process::exit(2);
    };
    let set = args.iter().position(|a| a == "--set").map(|i| (args.get(i + 1).cloned(), args.get(i + 2).cloned()));
    let mut conn = match Connection::tcp(addr.as_str(), Duration::from_secs(5)) {
        Ok(c) => c,
        Err(e) => {
            println!("cannot connect: {} ({})", e.message, e.message_key);
            std::process::exit(1);
        }
    };
    let (hello, n) = match conn.negotiate(&["settings"]) {
        Ok(x) => x,
        Err(e) => {
            println!("cannot talk to this machine: {}", e.message);
            std::process::exit(1);
        }
    };
    println!("Machine {} speaks contract {}.{}", hello.machine, hello.contract.major, hello.contract.minor);
    if !n.usable.iter().any(|d| d == "settings") {
        println!("This machine has no settings screen to show.");
        return;
    }
    let remote = RemoteSettings::new(conn);
    // the identity is the machine's to decide; the UI passes an empty local one only because the trait asks
    let ctx = CallContext::local("", false);
    let schema = match remote.schema(&ctx) {
        Ok(s) => s,
        Err(e) => {
            println!("Settings: {} [{}]", e.message, e.code.as_str());
            std::process::exit(1);
        }
    };
    for spec in &schema {
        match remote.get(&ctx, &spec.key) {
            Ok(v) => println!("{} = {}", spec.key, show(&v)),
            Err(e) => println!("{} = (not shown: {})", spec.key, e.code.as_str()),
        }
    }
    if let Some((Some(key), Some(value))) = set {
        let Some(spec) = schema.iter().find(|s| s.key == key) else {
            println!("There is no setting called {key}.");
            std::process::exit(1);
        };
        let Some(v) = parse(&spec.kind, &value) else {
            println!("{value:?} does not fit {key}.");
            std::process::exit(1);
        };
        match remote.set(&ctx, &key, v, None) {
            Ok(c) => println!("Set {} = {} (event {})", c.key, show(&c.value), c.seq),
            Err(e) => {
                println!("Refused: {} [{} / {}]", e.message, e.code.as_str(), e.message_key);
                std::process::exit(1);
            }
        }
    }
}
