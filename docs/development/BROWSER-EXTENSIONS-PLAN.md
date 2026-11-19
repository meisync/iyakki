# Browser extensions: plan (owner request 2026-11-11)

Status: **the mechanism exists for one hook kind (`request.headers`); the header rules are the first built-in extension.** Everything else here is a plan, not built.
The full-engine path of the header rules is written but has not run in a real WPE (KI-086, alpha.3 item 13).

## 1. What exists today

| Piece | Where |
|---|---|
| Snapshot layout, hook kinds, reference matcher | `crates/iyakki-webview/src/ext.rs` |
| Transport: optional message `Extensions` (0x16), opaque bytes, kept and re-sent after a helper restart | `proto.rs`, `client.rs`, `docs/WEBVIEW-PROTOCOL.md` |
| Registry and broker gate: `BrowserExtension`, `CapGate`, `BrowserExtensionHost`, `HeaderRulesExtension` | `crates/iyakki-agent/src/browserext.rs` |
| First extension: header rules (vault values, masked lists, `FilterSet` matching) | `crates/iyakki-agent/src/browserheaders.rs`, shell `apps/browser_headers.rs` |
| Helper: keeps the snapshot, sends it to web processes, logs the report | `dist/webview/wpe-helper/helper.c` |
| Web process extension and its parser/matcher in plain C | `dist/webview/wpe-helper/webext.c`, `extsnap.h`, test `ext-check.c` |
| Capability `web.request.headers` | `crates/iyakki-core-api/src/appcaps.rs` (catalogue entry; built-in table entry `browser-headers`) |

### The flow

1. An extension registers with the host (built-in ones at start). It declares its capabilities and, on request, its hooks (kind + payload bytes).
2. The shell builds a **snapshot**: for each extension the broker is asked for every declared capability (as a built-in app of the signed-in person). Only the granted capabilities are
   listed in the entry, and a hook is kept only if its kind's capability (`hook::capability`) is granted. An extension without its capability gets no hook (and no entry).
3. The snapshot goes to the helper (`ToHelper::Extensions`) when the full engine starts a page load and the host's generation changed (a rule was edited, the person signed in, the gate changed).
   Secrets stay in the vault; the payload is built from what the shell already holds in memory and travels only over the helper's socket. `Debug` of every type shows sizes and names, never values.
4. The helper stores it and tells the web processes: a `WebKitUserMessage` to running ones, the initialization user data for a web process that starts later.
5. The web process extension parses it, **dispatches per hook kind**, and answers with a report line ("loaded, 3 header rules: ...") that the helper logs. Unknown kinds, kinds without code,
   hooks without their capability and broken payloads are skipped and named in the report; a snapshot of another layout version is refused whole and clears the rules (fail closed).

**Versioning rules.** The snapshot has its own layout number (1). A new hook kind needs no new message and no change of the helper protocol version: it is a new number inside the bytes.
Adding fields to an existing payload means a new kind (or a new snapshot version, refused cleanly by older helpers). The transport never looks inside the bytes.

### Request headers, as built

`send-request` runs for the page's own requests, its sub-resources and every redirect hop. The extension adds the matching headers (same filter entries as the VPN and SSH tunnels, exported by
`FilterSet::name_patterns`), keeps a header the page already set (unless the rule says replace; no rule says so yet), adds at most 16, and one per header name (first rule wins). On a redirect hop it
first removes the headers it added earlier when the new address does not match, because the engine copies the previous request's headers into the redirected one. Narrower than the in-house engine by design:
a rule with a path regex or an address entry is not sent; plain http only to `localhost` names; an ambiguous path (escape, backslash, dot segment, empty segment) matches only host-only entries.
Unverified: service workers and shared workers probably do not pass through `send-request` and so get no headers.

## 2. Hook kinds

| Kind | Number | Capability (proposed unless noted) | Risk class | Where it runs | Built |
|---|---|---|---|---|---|
| `request.headers` | 1 | `web.request.headers` (in the catalogue) | medium: sends the person's secrets to named sites; filter must name sites, never a catch-all | web process, `send-request` | yes, unproven in WPE |
| `response.headers` | 2 | `web.response.headers` | **hard ask**: can remove security headers (CSP, frame options) | network process or web process (needs a design check: `send-request` sees requests only) | no |
| `request.block` | 3 | `web.request.block` | low: only removes requests; shows a count; can break pages | web process `send-request` (return TRUE with an empty request) or the network layer | no |
| `content.script` | 4 | `web.content.script` | **hard ask**: runs the author's code in the page with the page's cookies and form fields | `WebKitUserContentManager` user scripts (start/end of document, by URL pattern) | no |
| `page.action` | 5 | `web.page.action` | low to medium: a button the person presses; it receives the page's URL/title/selection | shell side (a toolbar button) plus a request to the extension | no |

Hard asks follow `appcaps`: every use shows a card, whatever was approved at install. Content scripts and response rewriting are hard asks because they can read or change what the person sees and types.
The catalogue gets a name only when its hook has code (version 0 is unstable until the beta tag, so proposed names can still change).

## 3. Manifest sketch

An extension is a member of an app bundle (`CORE-ACCESS-MODEL.md` section 6d: `contents`, one manifest, one signature, one install card, approval per bundle and member):

```json
{
  "id": "example.tracker-blocker",
  "version": "1.2.0",
  "contents": [
    {
      "kind": "extension",
      "file": "extension.json",
      "capabilities": [
        { "name": "web.request.block", "why": "hide ads and trackers", "param": "*.ads.example, *.track.example" }
      ]
    }
  ]
}
```

and `extension.json`:

```json
{
  "format": 1,
  "name": "Tracker blocker",
  "engines": ["inhouse", "full"],
  "hooks": [
    { "kind": "request.block", "match": ["*.ads.example", "https://example.org/pixel"], "except": ["ok.ads.example"] }
  ]
}
```

Rules: the hooks a file declares must be covered by the capabilities of its member entry (otherwise install is refused with a reason); `match` entries use the `FilterSet` syntax; secrets are never in the file
(a header value is a vault reference the person fills in at install); the size is capped like a tile bundle. A built-in extension is the same thing compiled in, listed in the signed built-in table.
Whether `engines` is needed at all, or the card just says "works in the full engine only", is open.

## 4. Matching

One matcher for everything: the `FilterSet` of `iyakki-vpn` (domain, `*.suffix`, URL prefix by whole path segments, optional scheme and port). Exclusions and address ranges are not used for headers; a future
`request.block` wants exclusions (allow lists), which `FilterSet` already has, so the wire pattern gets an `include` flag then (a new payload kind or version). The same vectors test the Rust matcher in the
agent, the Rust twin in `ext.rs` and the C twin in `extsnap.h` (`browserext_tests.rs`), so the three cannot drift unnoticed. Anything the C side cannot judge is not sent: it errs toward not adding a header.

## 5. Signing and trust

Like store packages (`docs/STORE.md`): a bundle is signed as a whole; the install card lists each member with its capabilities; the broker records an approval per `<bundle>/<member>`; an update shows only what is new.
Built-in extensions are covered by the image signature through the built-in table. Unsigned local extensions: developer mode only, with a banner, never on a managed (enterprise or cloud) instance.
Extension code that runs in the page (`content.script`) is JavaScript the author wrote: it is shown on the card as size and hash, and the card says plainly what a script can do. No remote code: a script is part of the signed
file, never fetched at run time.

## 6. WebExtensions compatibility (no promise)

A subset could be honoured later so existing add-ons load without a rewrite. Candidates, easiest first: `declarativeNetRequest` (block/redirect/modify headers by rules, maps to `request.block` and
`request.headers`), `content_scripts` in the manifest (maps to `content.script`), `webRequest` blocking is **not** a target (it runs arbitrary code per request; too slow and too powerful), `storage.local`, `action`/`browser_action`
(maps to `page.action`), `permissions`/`host_permissions` (map to capabilities plus a `FilterSet`). Not in scope: background pages, `tabs`/`windows` APIs beyond the active page, native messaging, `cookies` (separate
rule add-on instead). A converter that reads a Manifest V3 file and refuses what it cannot honour is smaller and safer than a runtime that pretends. The question for the owner is whether this is worth doing at all
versus a small set of first-party add-ons (section 8).

## 7. Behaviour per engine

| | In-house engine | Full web engine (WPE) | Client browser (the person's own browser, CLIENT-SPLIT) |
|---|---|---|---|
| Headers | yes, all rules (`headerfetch`), verified by tests | the subset described above, via the web process extension; unproven on a device | **does not apply**: the page runs in the user's own browser; our hooks are not there |
| Block | easy: the transport can refuse (the browser already has an ad and tracker blocker) | `send-request` | does not apply (the user's own blocker does the job) |
| Content scripts | not possible (no JavaScript engine of ours runs page scripts in the in-house engine) | yes, user scripts | does not apply |
| Response rewriting | possible in the transport | hard: needs the network process | does not apply |

A client device that shows our browser tile as pixels (pixel streaming) is the instance's engine, so the extensions run on the instance, not in the client's browser. The extension points never reach
a page the person opens in their own browser through the connector; saying this on the Settings page is part of any future extension UI.

## 8. Candidate built-in add-ons

Ranked by value divided by new code on the hook kinds already defined. "In-house first" marks what could be built in the in-house engine's transport before the full engine is proven. Effort: S under a few days, M about a week, L more. Not implemented; the header rules are the only one built.

| # | Add-on | What it does | Hook kinds | Risk class / capabilities | Full engine needed | In-house first | Effort |
|---|---|---|---|---|---|---|---|
| 1 | **Custom request headers** (built) | tokens, test headers per site | `request.headers` | medium, `web.request.headers` | no (works in-house); full path unproven | done | done |
| 2 | **HTTPS-only upgrade** | request https for an http address, fall back with a notice | `request.block` (rewrite) or a transport rule | low, `web.request.block` | no | yes | S |
| 3 | **Content and tracker blocker by domain rules** | the existing block lists, exposed as an extension with per-site allow | `request.block` | low, `web.request.block` | no (full engine for parity) | yes (exists today in-house) | S in-house, M for the full engine hook |
| 4 | **Per-site permissions** (deny camera, microphone, location, notifications by default per site) | the helper already asks per request; add per-domain defaults | none new: the shell's permission policy, a rule table | low to medium, uses `camera.view`/`microphone.listen` decisions already in the shell | yes (the full engine asks) | n/a | S |
| 5 | **User-agent and Accept-Language per domain** | override per site | `request.headers` with `replace` (UA also needs the engine setting for `navigator.userAgent`) | medium, `web.request.headers` | partly (UA string in JS needs the engine) | yes for headers | S |
| 6 | **Referrer policy per domain** | strip or trim `Referer` | `request.headers` (remove) | low, `web.request.headers` | no | yes | S (needs "remove" in the payload: new kind or version) |
| 7 | **Redirect rules** | send address A to B (mirror, privacy front-end) | `request.block` extended with a target | medium: can send the person elsewhere; shown in the card | no | yes | S to M |
| 8 | **Per-site audio mute and volume** | links to the web mixer (per-source volume exists) | `page.action` plus the mixer, no web process code | low, `speaker.play` | yes (the full engine has the sound) | n/a | S |
| 9 | **Send page to the assistant** | a button that gives the assistant the URL, title and selection | `page.action` | medium: page content leaves the page for the assistant; shows what is sent | no (both engines) | yes | S |
| 10 | **Download rules** (type, size, destination) | accept, refuse, or redirect by rule | none new: the download event already reaches the shell | low to medium, `files.write` with a folder | yes | n/a | S to M |
| 11 | **Dark mode, force zoom, font size per site** | per-domain CSS or zoom | `content.script`-lite (a style, not code) or the engine's zoom | style only: low; scripts: hard ask | zoom no; CSS yes | zoom yes | S (zoom), M (CSS) |
| 12 | **Reader mode** | the in-house reader as a button on the full engine | `page.action` | low | yes (the reader exists in-house) | exists | S |
| 13 | **Print or save page to PDF** | engine print path | `page.action` | low, `files.write` | yes | no | M |
| 14 | **Cookie rules per domain** | refuse, session-only, delete on close | none new: the cookie manager (WebKit `WebKitCookieManager`) plus a rule table; a transport rule in-house | medium | yes for the full engine | yes | M |
| 15 | **Page translate** (local model via the assistant) | translate the visible text | `content.script` or `page.action` plus the assistant; needs text in and out | medium: page text goes to the local model; hard ask if it injects script | yes | reader only | L |
| 16 | **Userscripts** (Greasemonkey style) | the person's own JavaScript per URL pattern | `content.script` | **hard ask**: code with the page's cookies and form fields | yes | no | M (hook) plus L (safety UI, editor) |
| 17 | **Password and passkey autofill from the person's vault** | fill forms from the vault | `content.script` plus `page.action`, vault read per use | **hard ask** per use; the most sensitive item here | yes | no | L |

Reading the table: 2, 3, 4, 6, 8, 9, 12 are small and mostly need no new hook kind. 5 and 6 need "replace/remove" in the header payload (a version bump of that payload only). 15 to 17 depend on `content.script`
and a decision on what a script may touch; they come last and each is a hard ask on every use.

## 9. Open questions

1. Is a store-installable extension (third-party code, signed bundle) wanted at all, or only first-party built-ins plus the person's own rules? This decides whether sections 3 and 5 are built.
2. Does the payload for headers get `remove` and `replace` per rule (UI: "Replace if the page sets it", "Remove this header")?
3. Where do `response.headers` run? `send-request` is request-only; the response side needs the network process or a different WPE hook. Needs a check against the real API.
4. Do worker requests (service worker, shared worker) pass through `send-request`? If not, the Settings page says so.
5. The access log: the host's gate uses a null sink today (no log line per snapshot build); should a granted or refused extension capability appear in the access log with the extension id?
6. Should the full engine honour path-regex rules by shipping the in-house regex engine to C, or by evaluating them in the shell for the main frame only?
7. Is WebExtensions compatibility (section 6) wanted, or a first-party add-on list?
8. Per-profile or per-person: rules are per person today; a shared (family) profile needs a policy.
