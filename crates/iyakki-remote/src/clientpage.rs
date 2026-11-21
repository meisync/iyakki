//! The page behind a client's one-time link (`/connect`), served by the instance: HTML, a style sheet, the page script, the glue and the
//! built client engine (WebAssembly). EXPERIMENTAL and off unless the setting is on (the server answers 404 for every one of these paths
//! while it is off). Everything is embedded; the page makes no request to any other address, which the content security policy enforces.
//!
//! The engine and its glue come from `iyakki-client-engine`; the module is committed and rebuilt with `dist/client-engine/build.sh`.

use crate::http::Response;

const INDEX_HTML: &str = include_str!("clientpage/index.html");
const PAGE_JS: &str = include_str!("clientpage/page.js");
const PAGE_CSS: &str = include_str!("clientpage/page.css");

/// No inline script or style, nothing from outside, WebAssembly compiled from our own file only, the connection to the same origin only,
/// no framing, no forms posted anywhere. `ws:`/`wss:` to the same host are covered by `'self'` in current browsers.
pub const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; connect-src 'self'; img-src 'self' data:; font-src 'none'; media-src 'none'; object-src 'none'; worker-src 'none'; manifest-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The paths of the page, for tests and the router.
pub const PATHS: [&str; 10] = ["/connect", "/connect/page.js", "/connect/page.css", "/connect/engine.js", "/connect/keys.js", "/connect/engine.wasm", "/connect/media.js", "/connect/audio-worklet.js", "/connect/uidisplay.js", "/connect/files.js"];

/// The page may use this device's camera and microphone and the clipboard only for itself, and only when the person presses a button (the
/// browser asks too); nothing else.
pub const PERMISSIONS_POLICY: &str = "camera=(self), microphone=(self), geolocation=(), clipboard-read=(self), clipboard-write=(self)";

fn secure(r: Response) -> Response {
    r.header("Content-Security-Policy", CSP).header("Referrer-Policy", "no-referrer").header("Cache-Control", "no-store").header("Permissions-Policy", PERMISSIONS_POLICY).header("Cross-Origin-Opener-Policy", "same-origin")
}

/// The response for `path` (a GET), or `None` when the page has no such file.
pub fn serve(path: &str) -> Option<Response> {
    let js = "text/javascript; charset=utf-8";
    Some(secure(match path {
        "/connect" => Response::html(INDEX_HTML),
        "/connect/page.js" => Response::new(200, js, PAGE_JS),
        "/connect/page.css" => Response::new(200, "text/css; charset=utf-8", PAGE_CSS),
        "/connect/engine.js" => Response::new(200, js, iyakki_client_engine::GLUE_JS),
        "/connect/keys.js" => Response::new(200, js, iyakki_client_engine::KEYS_JS),
        "/connect/uidisplay.js" => Response::new(200, js, iyakki_client_engine::UIDISPLAY_JS),
        "/connect/files.js" => Response::new(200, js, iyakki_client_engine::FILES_JS),
        "/connect/media.js" => Response::new(200, js, iyakki_client_engine::MEDIA_JS),
        "/connect/audio-worklet.js" => Response::new(200, js, iyakki_client_engine::WORKLET_JS),
        "/connect/engine.wasm" if !iyakki_client_engine::WASM.is_empty() => Response::new(200, "application/wasm", iyakki_client_engine::WASM),
        _ => return None,
    }))
}
