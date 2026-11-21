# Browser plug-ins (header modifier and page tools)

Status: design agreed in conversation 2026-11-06. Written by 2026-11-19: the registry and permission cards (Rust, alpha.7 lane d1), the header modifier (web process extension, F-BROW-79) and, in the final Impl wave, the helper's half: user scripts, user styles and the User-Agent read from the snapshot (`dist/webview/wpe-helper/extsnap.h` parses them, `helper.c` `apply_plugins` puts them in the content manager and on the live settings). The cookie editor and the JSON viewer of the first set are not written. Not run against a real engine. Related: [`../FULL-BROWSER.md`](../FULL-BROWSER.md), `crates/iyakki-agent/src/browserheaders.rs`, `crates/iyakki-apps/src/apps/browser_headers.rs`, `docs/STORE.md` (package kind `extension`).

## What exists

* **Header rules** for the built-in reader: per-person rules (domain filter, URL prefix and, since 2026-11-06, an optional regular expression on the path and query, matched by the in-house engine with a step limit; the domain or prefix is still required), values in the person's vault, refusals for headers the browser owns, a "I know" tick for Cookie and Authorization. The full web engine does **not** receive them (stated in `browser_headers.rs`).
* Dark mode, content blocker, reader mode, saved passwords and the developer panel.

## Why they are our own plug-ins

WPE WebKit has no WebExtensions API, so Chrome and Firefox extensions cannot run. Plug-ins are IYAKKI page tools: small scripts, styles and request hooks the web view helper applies, installed from Quick install or the Store as packages (kind `extension`, target `browser`).

## First set (owner to confirm)

1. **Header modifier on the full engine.** Same rules and vault as today.
2. **User-Agent switcher.** Presets and the person's own string; per person, not per tab.
3. **Cookie editor** for the current site.
4. **User scripts and styles** per site (injected at document start or end).
5. **JSON viewer** for pages that answer with JSON.

## Engine side (what needs the helper, `dist/webview/wpe-helper/helper.c`)

* **User-Agent:** already supported at start (`G.ua`, from Hello). A new message `T_USER_AGENT` sets it on the live view (`webkit_settings_set_user_agent`) and reloads. Applies to every request of the page.
* **Headers, top-level navigations:** in `on_policy` (NAVIGATION_ACTION), when a rule matches the host, ignore the decision and `webkit_web_view_load_request` with the rule's headers added to `webkit_uri_request_get_http_headers`; a flag keyed by the URL stops a loop. Works for the page itself, not for the images, scripts and `fetch` calls it makes.
* **Headers, sub-resources:** only possible from a web process extension (`WebKitWebPage::send-request`), a small shared object the helper loads, like the camera plug-in already is. Without it the tool says plainly "top-level pages only".
* **Scripts and styles:** `webkit_user_content_manager_add_script` / `add_style_sheet`, the way `media-probe.js` and `dev-probe.js` already are.
* The rules cross to the helper as a new protocol message (`T_HEADERS`, JSON, size-capped); values are read from the vault by the shell and never written to a file or a log. `docs/WEBVIEW-PROTOCOL.md` gets the message.

## Limits to state in the tool, not hide

* Top-level only until the extension exists.
* A header the engine's network stack owns (Host, Content-Length, Cookie when the engine has its own jar) is refused, as in the reader.
* HTTPS pages: the rule applies to the request the engine sends, and the person sees which rules matched on the page's status line.

## Tests to write with it

Rule matching (shared with the reader); the protocol message round trip; the loop guard; refusals; a fake-helper test of "the helper got the rules and not the values in any log"; a run against a local HTTP server that echoes headers (QEMU or the Mac) for the real engine; UA string reaching the server.

## What the helper does with the snapshot (written 2026-11-19)

* `content.script` (hook kind 4, capability `web.content.script`): up to 16 entries, each a script or a style sheet with at most 32 KiB, `at_end`, and the sites it runs on. The sites become the engine's own match patterns (`https://*.example.com/*`, one per scheme; a plug-in's `*.example.com` means the domain and its subdomains, which is what the engine's `*.` pattern matches); a site with a port is left out (the engine's patterns have none, so it could only match more); an entry with no usable site is dropped and named, never run on every site. Scripts and styles go in the top frame only. The previous snapshot's entries are removed first, so a plug-in switched off is gone from the next page.
* `user.agent` (hook kind 6, capability `web.user.agent`): one line of printable ASCII, at most 400 characters, set with `webkit_settings_set_user_agent`; empty or absent returns to the engine's own. It applies to the next page (the shell sends the snapshot before each page).
* Parsing is plain C (`extsnap.h`) and is tested with the host's compiler against the Rust encoder (`iyakki-agent::browserext_tests`, `content-check.c`).

## Not done because

The helper is C against WPE WebKit and cannot be compiled on the development PC (no WPE headers); it builds inside the image pipeline. Write and review it together with one incremental rebuild of the `iyakki-webview-helper` package before a full image.
