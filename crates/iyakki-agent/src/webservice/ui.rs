//! The web UI: a few static files (plain HTML, CSS and JS, our own, no outside library) served at `/ui/`. They are compiled into
//! the binary, need no sign-in (they hold nothing private) and talk to the same `/api/v1/` routes as any other client, with the
//! bearer token of a normal sign-in. The server checks every call; the pages only hide what the person may not do.
//!
//! Plan: `docs/development/WEB-UI-PLAN.md`. These files are not in the route table because they are not API; they answer only
//! GET, and every other path under `/ui/` is a 404. The strict Content-Security-Policy below is the reason the pages have
//! no inline script or style and make no request to any other address.

use crate::httpmini::{Request, Response};

/// Everything the page may load comes from this origin; nothing is inline, nothing is framed, no form leaves the page.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// (path, content type, body).
pub const ASSETS: &[(&str, &str, &str)] = &[
    ("/ui/", "text/html; charset=utf-8", include_str!("ui/index.html")),
    ("/ui/app.css", "text/css; charset=utf-8", include_str!("ui/app.css")),
    ("/ui/app.js", "text/javascript; charset=utf-8", include_str!("ui/app.js")),
    ("/ui/common.js", "text/javascript; charset=utf-8", include_str!("ui/common.js")),
    ("/ui/files.js", "text/javascript; charset=utf-8", include_str!("ui/files.js")),
    ("/ui/git.js", "text/javascript; charset=utf-8", include_str!("ui/git.js")),
    ("/ui/flows.js", "text/javascript; charset=utf-8", include_str!("ui/flows.js")),
    ("/ui/runs.js", "text/javascript; charset=utf-8", include_str!("ui/runs.js")),
    ("/ui/schedules.js", "text/javascript; charset=utf-8", include_str!("ui/schedules.js")),
    ("/ui/tenant.js", "text/javascript; charset=utf-8", include_str!("ui/tenant.js")),
    ("/ui/cloud.js", "text/javascript; charset=utf-8", include_str!("ui/cloud.js")),
    ("/ui/remote.js", "text/javascript; charset=utf-8", include_str!("ui/remote.js")),
    ("/ui/notifications.js", "text/javascript; charset=utf-8", include_str!("ui/notifications.js")),
    ("/ui/bell.js", "text/javascript; charset=utf-8", include_str!("ui/bell.js")),
    ("/ui/tilerender.js", "text/javascript; charset=utf-8", include_str!("ui/tilerender.js")),
    ("/ui/tiles.js", "text/javascript; charset=utf-8", include_str!("ui/tiles.js")),
    ("/ui/tenantcfg.js", "text/javascript; charset=utf-8", include_str!("ui/tenantcfg.js")),
];

/// The answer for a request under `/ui/`, or `None` when the path is not one of ours (the caller answers 404).
pub fn serve(req: &Request) -> Option<Response> {
    if req.method != "GET" {
        return None;
    }
    // `/ui` without the slash is what people type: send them to the page (a 404 there looked like "the UI is not installed")
    if req.path == "/ui" {
        return Some(Response::new(308, "text/plain; charset=utf-8", Vec::new()).header("Location", "/ui/").header("Cache-Control", "no-store"));
    }
    let path = if req.path == "/ui/index.html" { "/ui/" } else { req.path.as_str() };
    let (_, ctype, body) = ASSETS.iter().find(|(p, _, _)| *p == path)?;
    Some(
        Response::new(200, ctype, body.as_bytes().to_vec())
            .header("Content-Security-Policy", CSP)
            .header("X-Content-Type-Options", "nosniff")
            .header("Referrer-Policy", "no-referrer")
            .header("Cross-Origin-Opener-Policy", "same-origin")
            .header("Cache-Control", "no-store"),
    )
}

/// Is this path in the UI's own area (so that a miss is a UI 404 and not an API one)?
pub fn is_ui_path(path: &str) -> bool {
    path == "/ui" || path.starts_with("/ui/")
}
