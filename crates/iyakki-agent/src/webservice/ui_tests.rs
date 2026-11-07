//! The web UI: its static files (headers, no outside request, no inline script, licence header) and the routes its two first pages
//! use that are new (Remote access, the member list of a namespace), with the refusals the server must give whatever the page hides.

use super::*;
use crate::webservice::ui;

fn header<'a>(r: &'a Response, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

// ------------------------------------------------------------------------------------------------ static files

#[test]
fn the_ui_is_served_without_sign_in_with_a_strict_policy_on_every_file() {
    for p in ["agent-api", "cloud"] {
        let h = harness(p, |_| {});
        for (path, ctype, body) in ui::ASSETS {
            let (s, _, resp) = h.call("GET", path, None, None);
            assert_eq!(s, 200, "{path}");
            assert_eq!(header(&resp, "Content-Type"), Some(*ctype), "{path}");
            assert_eq!(resp.body, body.as_bytes(), "{path}");
            assert_eq!(header(&resp, "Content-Security-Policy"), Some(ui::CSP), "{path}");
            assert_eq!(header(&resp, "X-Content-Type-Options"), Some("nosniff"));
            assert_eq!(header(&resp, "Cache-Control"), Some("no-store"));
            assert_eq!(header(&resp, "Referrer-Policy"), Some("no-referrer"));
        }
        assert_eq!(h.call("GET", "/ui/index.html", None, None).0, 200);
    }
}

#[test]
fn the_policy_allows_only_this_origin_and_no_inline_code() {
    let csp = ui::CSP;
    assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'") && csp.contains("style-src 'self'") && csp.contains("connect-src 'self'"));
    assert!(csp.contains("frame-ancestors 'none'") && csp.contains("base-uri 'none'"));
    for bad in ["unsafe-inline", "unsafe-eval", "http:", "https:", "*", "data:script"] {
        assert!(!csp.replace("img-src 'self' data:", "").contains(bad), "{bad}");
    }
}

#[test]
fn only_get_and_only_the_listed_files_exist_under_ui() {
    let h = harness("agent-api", |_| {});
    for (m, p) in [("POST", "/ui/"), ("PUT", "/ui/app.js"), ("DELETE", "/ui/app.css"), ("GET", "/ui/nothing.js"), ("GET", "/ui/../api/v1/version"), ("GET", "/ui/%2e%2e/x")] {
        let (s, v, _) = h.call(m, p, None, Some(json!({"x": 1})).filter(|_| m != "GET"));
        assert_eq!(s, 404, "{m} {p}");
        assert_eq!(v["error"]["code"], "not_found");
    }
    // `/ui` without the slash goes to the page (it used to be a 404, which looked like a missing UI)
    let (s, _, resp) = h.call("GET", "/ui", None, None);
    assert_eq!(s, 308);
    assert_eq!(header(&resp, "Location"), Some("/ui/"));
    assert_eq!(h.call("POST", "/ui", None, Some(json!({"x": 1}))).0, 404, "only GET is redirected");
    // the API is untouched
    assert_eq!(h.call("GET", "/api/v1/version", None, None).0, 200);
    assert_eq!(h.call("GET", "/api/v1/whoami", None, None).0, 401);
}

#[test]
fn the_files_have_no_outside_address_no_inline_script_and_no_unsafe_calls() {
    for (path, _, body) in ui::ASSETS {
        for ext in ["http://", "https://", "ws://", "wss://", "ftp://", "//cdn", "@import", "url(http"] {
            assert!(!body.contains(ext), "{path} mentions {ext}");
        }
        for unsafe_ in ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "eval(", "new Function", "javascript:", "WebSocket(", "localStorage", "document.cookie", "importScripts", "<iframe", "<object", "<embed"] {
            assert!(!body.contains(unsafe_), "{path} uses {unsafe_}");
        }
        // fetch cannot report upload progress: XMLHttpRequest is allowed in the Files page only (same origin, the CSP's connect-src)
        assert!(!body.contains("XMLHttpRequest") || *path == "/ui/files.js", "{path} uses XMLHttpRequest");
        // an absolute path is only ever one of ours
        for (i, _) in body.match_indices("/api/") {
            assert!(body[i..].starts_with("/api/v1"), "{path}: {}", &body[i..(i + 20).min(body.len())]);
        }
    }
    let html = ui::ASSETS.iter().find(|a| a.0 == "/ui/").unwrap().2;
    // every script is a file of ours; there is no inline script, no inline style, no inline handler
    let scripts: Vec<&str> = html.match_indices("<script").map(|(i, _)| &html[i..i + html[i..].find('>').unwrap()]).collect();
    let n_js = ui::ASSETS.iter().filter(|a| a.0.ends_with(".js")).count();
    assert_eq!(scripts.len(), n_js, "every script file is loaded by the page, and the page loads no other");
    for s in &scripts {
        assert!(s.contains("src=\"/ui/") && !s.contains("type=\"text/javascript\"") || s.contains("src=\"/ui/"), "{s}");
    }
    assert!(!html.contains("<style") && !html.contains(" style=") && !html.contains("<link rel=\"stylesheet\" href=\"http"));
    for h in [" onclick=", " onload=", " onerror=", " onsubmit=", " onfocus=", " oninput=", " onchange="] {
        assert!(!html.contains(h), "{h}");
    }
    // the stylesheet and the scripts named by the page are the ones served
    for name in ["/ui/app.css", "/ui/app.js", "/ui/common.js", "/ui/files.js", "/ui/git.js", "/ui/flows.js", "/ui/runs.js", "/ui/cloud.js", "/ui/remote.js", "/ui/tenant.js", "/ui/tenantcfg.js"] {
        assert!(html.contains(&format!("\"{name}\"")), "{name}");
        assert!(ui::ASSETS.iter().any(|a| a.0 == name));
    }
}

#[test]
fn the_page_follows_the_colour_scheme_and_has_big_targets() {
    let css = ui::ASSETS.iter().find(|a| a.0 == "/ui/app.css").unwrap().2;
    assert!(css.contains("prefers-color-scheme: dark") && css.contains("color-scheme: light dark"));
    assert!(css.contains("min-height: 44px") && css.contains(":focus-visible"));
}

// ------------------------------------------------------------------------------------------------ Remote access

#[test]
fn remote_access_is_for_administrators_and_changes_are_audited_like_the_native_page() {
    let h = harness("agent-api", |_| {});
    let (root, ann) = (h.root(), h.ann());
    // reading
    let (s, v, _) = h.call("GET", "/api/v1/webaccess", Some(&root), None);
    assert_eq!((s, v["on"].clone(), v["port"].clone()), (200, json!(false), json!(8443)), "{v}");
    assert!(v["exposes"].as_str().unwrap().contains("Anyone on this network"));
    // refusals: nobody, a plain user, a guest; and nothing changed
    let gus = h.login("gus", "gus-password-1");
    assert_eq!(h.call("GET", "/api/v1/webaccess", None, None).0, 401);
    assert_eq!(h.call("POST", "/api/v1/webaccess", None, Some(json!({"on": true}))).0, 401);
    for t in [&ann, &gus] {
        assert_eq!(h.call("GET", "/api/v1/webaccess", Some(t), None).0, 403);
        assert_eq!(h.call("POST", "/api/v1/webaccess", Some(t), Some(json!({"on": true}))).0, 403);
        assert_eq!(h.call("POST", "/api/v1/webaccess", Some(t), Some(json!({"port": 9000}))).0, 403);
    }
    assert_eq!(h.call("GET", "/api/v1/webaccess", Some(&root), None).1["on"], false);
    assert!(!h.accounts.lines().iter().any(|l| l.contains("settings.web_access")));
    // switching on and the port
    assert_eq!(h.call("POST", "/api/v1/webaccess", Some(&root), Some(json!({"on": true}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/webaccess", Some(&root), Some(json!({"port": 9443}))).0, 200);
    let (_, v, _) = h.call("GET", "/api/v1/webaccess", Some(&root), None);
    assert_eq!((v["on"].clone(), v["port"].clone(), v["urls"][0].as_str()), (json!(true), json!(9443), Some("https://x.local:9443")));
    let lines = h.accounts.lines();
    assert!(lines.iter().any(|l| l == "root settings.web_access on (port 8443, HTTPS)"), "{lines:?}");
    assert!(lines.iter().any(|l| l == "root settings.web_access.port 9443"), "{lines:?}");
    assert!(lines.iter().filter(|l| l.contains("api.webaccess_set")).count() == 2, "{lines:?}");
    assert_eq!(h.call("POST", "/api/v1/webaccess", Some(&root), Some(json!({"on": false}))).0, 200);
    assert!(h.accounts.lines().iter().any(|l| l == "root settings.web_access off (port 9443, HTTPS)"));
}

#[test]
fn remote_access_refuses_bad_requests_and_is_not_offered_on_a_pod() {
    let h = harness("agent-api", |_| {});
    let root = h.root();
    for body in [json!({}), json!({"port": 80}), json!({"port": 70000}), json!({"on": "yes"}), json!({"port": "9000"}), json!({"on": true, "https": false}), json!({"plain_http": true})] {
        assert_eq!(h.call("POST", "/api/v1/webaccess", Some(&root), Some(body.clone())).0, 400, "{body}");
    }
    assert_eq!(h.call("GET", "/api/v1/webaccess", Some(&root), None).1["on"], false, "nothing changed");
    let c = harness("cloud", |_| {});
    let t = c.root();
    assert_eq!(c.call("GET", "/api/v1/webaccess", Some(&t), None).0, 501);
    assert_eq!(c.call("POST", "/api/v1/webaccess", Some(&t), Some(json!({"on": true}))).0, 501);
    assert!(openapi::document(Profile::AgentApi)["paths"].get("/api/v1/webaccess").is_some());
    assert_eq!(openapi::document(Profile::AgentApi)["paths"]["/api/v1/webaccess"]["post"]["x-access"], "admin");
}

// ------------------------------------------------------------------------------------------------ Cloud access

#[test]
fn the_member_list_of_a_namespace_is_for_people_in_it() {
    let (h, root, ann, bob, gus) = acme_cloud();
    let (s, v, _) = h.call("GET", "/api/v1/namespaces/web/members", Some(&ann), None);
    assert_eq!((s, v["role"].as_str()), (200, Some("editor")), "{v}");
    let names: Vec<(&str, &str)> = v["members"].as_array().unwrap().iter().map(|m| (m["name"].as_str().unwrap(), m["role"].as_str().unwrap())).collect();
    assert_eq!(names, [("root", "admin"), ("ann", "editor")]);
    assert_eq!(h.call("GET", "/api/v1/namespaces/web/members", Some(&root), None).0, 200);
    // bob is in the tenant, not in web; gus is a stranger; nobody is signed in; no such namespace
    assert_eq!(h.call("GET", "/api/v1/namespaces/web/members", Some(&bob), None).0, 404, "the same answer as for a namespace that is not there");
    assert!([403, 404].contains(&h.call("GET", "/api/v1/namespaces/web/members", Some(&gus), None).0));
    assert_eq!(h.call("GET", "/api/v1/namespaces/web/members", None, None).0, 401);
    assert_ne!(h.call("GET", "/api/v1/namespaces/nope/members", Some(&ann), None).0, 200);
    // a tenant administrator sees the name of a namespace they are not in, not its members, until they join (recorded)
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "bob", "role": "admin"}))).0, 200);
    assert_eq!(h.call("GET", "/api/v1/namespaces/web/members", Some(&bob), None).0, 403);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&bob), Some(json!({"name": "bob", "role": "editor"}))).0, 200);
    assert_eq!(h.call("GET", "/api/v1/namespaces/web/members", Some(&bob), None).0, 200);
    assert!(openapi::document(Profile::Cloud)["paths"]["/api/v1/namespaces/{id}/members"].get("get").is_some());
}

#[test]
fn what_the_cloud_page_hides_the_server_still_refuses() {
    let (h, root, ann, _bob, _gus) = acme_cloud();
    // a namespace editor cannot manage members; a tenant member cannot manage the tenant or namespaces
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&ann), Some(json!({"name": "bob", "role": "viewer"}))).0, 403);
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&ann), Some(json!({"name": "ann", "role": "owner"}))).0, 403);
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&ann), Some(json!({"name": "x", "action": "create"}))).0, 403);
    // the view the page draws from: members only for administrators
    let (_, v, _) = h.call("GET", "/api/v1/tenant", Some(&ann), None);
    assert_eq!((v["tenant_role"].as_str(), v["members"].clone()), (Some("member"), json!([])));
    let (_, v, _) = h.call("GET", "/api/v1/tenant", Some(&root), None);
    assert_eq!(v["tenant_role"], "owner");
    assert_eq!(v["members"].as_array().unwrap().len(), 3);
}

// ------------------------------------------------------------------------------------------------ licence header

/// Every `.html`, `.js` and `.css` file of the UI carries the Apache-2.0 header, and every such file in the folder is served.
#[test]
fn every_ui_file_has_the_apache_header_and_every_file_in_the_folder_is_served() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let mut found = vec![];
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(["html", "js", "css"].contains(&p.extension().and_then(|x| x.to_str()).unwrap_or("")), "unexpected file {name}: serve it or move it");
        let text = std::fs::read_to_string(&p).unwrap();
        let head: String = text.chars().take(400).collect();
        assert!(head.contains("SPDX-License-Identifier: Apache-2.0") && head.contains("Licensed under the Apache License, Version 2.0"), "{name}: no Apache-2.0 header");
        found.push(name);
    }
    found.sort();
    let mut served: Vec<String> = ui::ASSETS.iter().map(|a| if a.0 == "/ui/" { "index.html".to_string() } else { a.0.trim_start_matches("/ui/").to_string() }).collect();
    served.sort();
    assert_eq!(found, served, "a file in ui/ is not served, or a served file is missing");
}

/// The shared file loads first and the pages before the shell, which reads the page registry once when it starts.
#[test]
fn the_scripts_load_in_the_order_the_registry_needs() {
    let html = ui::ASSETS.iter().find(|a| a.0 == "/ui/").unwrap().2;
    let at = |n: &str| html.find(&format!("\"/ui/{n}.js\"")).unwrap_or_else(|| panic!("{n}.js is not loaded"));
    assert!(at("common") < at("files") && at("files") < at("git"), "common first; git reads the spaces files.js shares");
    assert!(at("tenant") < at("app"), "the tenant control is there before the shell starts");
    for page in ["files", "git", "flows", "runs", "cloud", "remote"] {
        assert!(at("common") < at(page) && at(page) < at("app"), "{page} registers before the shell starts");
    }
}

/// A page file never asks for a secret and never writes one anywhere it could be kept.
#[test]
fn the_page_files_keep_nothing_but_the_tab_token_and_log_nothing() {
    for (path, _, body) in ui::ASSETS {
        for bad in ["console.log", "console.debug", "sessionStorage.setItem", "indexedDB", "navigator.clipboard", "type=\"password\""] {
            assert!(!body.contains(bad) || *path == "/ui/" || (*path == "/ui/app.js" && bad == "sessionStorage.setItem"), "{path} uses {bad}");
        }
    }
}

/// The Files, Git and Flows pages call the routes that now exist (move, mkdir, the four git reads, run and runs), keep the old `.keep`
/// folders working, and no longer carry a "planned" notice.
#[test]
fn the_files_git_and_flows_pages_use_the_new_routes_and_carry_no_planned_notice() {
    use super::routes::ROUTES;
    let body = |n: &str| ui::ASSETS.iter().find(|a| a.0 == format!("/ui/{n}.js")).unwrap().2;
    let has = |id: &str| ROUTES.iter().any(|r| r.id == id && r.in_profile(Profile::Cloud));
    for id in ["files_move", "files_mkdir", "git_status", "git_branches", "git_log", "git_diff", "flows_run", "flows_runs", "flows_run_get"] {
        assert!(has(id), "{id} is a route of the cloud profile");
    }
    let (f, g, l) = (body("files"), body("git"), body("flows"));
    assert!(f.contains("/api/v1/files/move") && f.contains("/api/v1/files/mkdir") && f.contains("MARKER"), "files.js uses move and mkdir and still hides .keep");
    assert!(!f.contains("content: \"\" }), \"Folder"), "a new folder is no longer a hidden file");
    for r in ["\"/api/v1/git/\" + tab", "status", "branches", "log", "diff"] {
        assert!(g.contains(r), "git.js: {r}");
    }
    assert!(l.contains("/run\"") && l.contains("/runs\" +") && l.contains("#/runs"), "flows.js starts runs, lists them and links to the Runs page");
    for (n, b) in [("git", g), ("flows", l)] {
        assert!(!b.contains("PLANNED") && !b.contains("Planned"), "{n}.js still says planned");
    }
    // no approve or deny call outside the Runs page
    for (n, b) in [("files", f), ("git", g), ("flows", l)] {
        assert!(!b.contains("/approvals"), "{n}.js must not answer questions");
    }
}
