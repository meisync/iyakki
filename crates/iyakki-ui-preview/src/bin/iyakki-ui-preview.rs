//! `iyakki-ui-preview [--port N] [--size WxH]`: serve the SAMPLE app (or the app named by `--app`) to a browser on 127.0.0.1.

use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use iyakki_app_sample::SampleApp;
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::wire::Session;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let val = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let port: u16 = val("--port").and_then(|p| p.parse().ok()).unwrap_or(0);
    let (w, h) = val("--size").and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))).unwrap_or((800u32, 480u32));
    if let Some(app) = val("--app") {
        if app != iyakki_app_sample::ID {
            eprintln!("only the app `{}` is built into the preview", iyakki_app_sample::ID);
            std::process::exit(2);
        }
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen: {e}");
            std::process::exit(1);
        }
    };
    let host = Host::new(SampleApp::new(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[iyakki_app_sample::ID])), Box::new(MemWallpaper::default()));
    println!("http://127.0.0.1:{}/", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    if let Err(e) = iyakki_ui_preview::serve(listener, Session::new(host), Arc::new(AtomicBool::new(false))) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
