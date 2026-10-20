//! `iyakki-store-server`: the private store server as a program of its own, so a static (musl) build for a `FROM scratch`
//! image is quick and small. It is the same code as `iyakki store-server` (which stays as an alias): a leading
//! `store-server` argument is accepted so the image's arguments work with either binary. See docs/STORE.md, section 13.

use std::process::ExitCode;

const USAGE: &str = "\
iyakki-store-server: the IYAKKI private app store (signed catalog, packages and publishing over HTTP)

USAGE:
  iyakki-store-server --data DIR [--listen 0.0.0.0:8080] [--publish-token-file F] [--catalog-key-file F | --catalog-seed-file F]
                      [--read-only] [--require-trusted] [--id ID] [--name N] [--max-body-mib N]
  iyakki-store-server publisher-add --data DIR (--catalog-key-file F | --catalog-seed-file F) --pub-id ID [--pub-name N]
  iyakki-store-server --version | --help

The same as `iyakki store-server`. Serves /healthz, /catalog.json and the packages; publishing needs a token (docs/STORE.md, section 13).
";

fn say(s: &str) {
    use std::io::Write;
    let _ = std::io::stdout().write_all(s.as_bytes());
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("store-server") {
        args.remove(0);
    }
    match args.first().map(String::as_str) {
        Some("--help" | "-h" | "help") => {
            say(USAGE);
            return ExitCode::SUCCESS;
        }
        Some("--version" | "-V" | "version") => {
            say(&format!("iyakki-store-server {}\n", env!("CARGO_PKG_VERSION")));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    match iyakki_agent::store::server_http::run_cli(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "iyakki-store-server: {e}");
            ExitCode::FAILURE
        }
    }
}
