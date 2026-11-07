//! The logic of the web UI's page files (`src/webservice/ui/{common,files,git,flows,runs}.js`) under node: path joining, breadcrumbs,
//! paging, role hiding, upload checks, clone checks, run events, the approval body, secret redaction, and the shell's "already
//! signed in to the desktop" probe. Same method as `webui_js.rs`: skipped (passes, saying so) when `node` is not installed; nothing
//! is installed or downloaded. This is NOT a real browser.

const DRIVER: &str = r##"
const path = require("path");
const dir = process.argv[2];
const App = require(path.join(dir, "app.js"));
const C = require(path.join(dir, "common.js"));
const F = require(path.join(dir, "files.js"));
const G = require(path.join(dir, "git.js"));
const L = require(path.join(dir, "flows.js"));
const R = require(path.join(dir, "runs.js"));
let bad = 0;
const ok = (c, m) => { console.log((c ? "OK  " : "BAD ") + m); if (!c) bad++; };
const j = JSON.stringify;

(async () => {
  // ---- common
  ok(C.formatSize(0) === "0 B" && C.formatSize(1023) === "1023 B" && C.formatSize(1536) === "1.5 KiB" && C.formatSize(5 * 1024 * 1024) === "5.0 MiB" && C.formatSize(-1) === "" && C.formatSize("x") === "", "sizes");
  ok(C.formatTime(1700000000) === "2023-11-14 22:13:20 UTC" && C.formatTime(1700000000000) === "2023-11-14 22:13:20 UTC" && C.formatTime(undefined) === "" && C.formatTime("soon") === "soon", "times in seconds or milliseconds");
  ok(C.clip("abcdef", 3).startsWith("abc ... (3 more") && C.clip("ab", 3) === "ab" && C.clip(null, 3) === "", "clip");
  ok(C.stateKind(200) === "ok" && C.stateKind(0) === "offline" && C.stateKind(401) === "signin" && C.stateKind(403) === "denied" && C.stateKind(404) === "missing" && C.stateKind(500) === "error", "state kinds");
  const reg = {}; C.registerPage(reg, { id: "a" }); C.registerPage(reg, { id: "b" });
  ok(reg.IyakkiPages.length === 2 && reg.IyakkiPages[1].id === "b", "pages register in order");
  const lt = C.latest(); const a1 = lt.next(); const a2 = lt.next(); ok(!lt.isCurrent(a1) && lt.isCurrent(a2), "only the latest load is drawn");
  const secret = "Authorization: Bearer abcdefgh12345678 password=hunter2 api_key: \"k-123\" sk-abcdefghijklmnop https://u:pw@host.example/x -----BEGIN RSA PRIVATE KEY-----\nAAA\n-----END RSA PRIVATE KEY-----";
  const red = C.redact(secret);
  ok(!/abcdefgh12345678|hunter2|k-123|sk-abcdefghijklmnop|:pw@|AAA/.test(red) && red.includes("[hidden]") && red.includes("host.example"), "secrets are hidden: " + red.replace(/\n/g, " "));
  ok(C.redact("plain text about a token of appreciation") === "plain text about a token of appreciation", "ordinary words are left alone");

  // ---- files
  ok(F.joinPath("", "a") === "a" && F.joinPath("a/b", "c.txt") === "a/b/c.txt" && F.joinPath("/a//b/", "c") === "a/b/c", "join");
  ok(F.parentPath("a/b/c") === "a/b" && F.parentPath("a") === "" && F.parentPath("") === "", "parent");
  ok(j(F.breadcrumbs("a/b", "Mine")) === j([{ label: "Mine", path: "" }, { label: "a", path: "a" }, { label: "b", path: "a/b" }]) && F.breadcrumbs("", "").length === 1, "breadcrumbs");
  ok(F.validName("a b.txt") && !F.validName("") && !F.validName("..") && !F.validName(".") && !F.validName("a/b") && !F.validName("a\\b") && !F.validName("a\nb") && !F.validName(" a") && !F.validName("x".repeat(201)), "names");
  const ents = [{ name: "b.txt", kind: "file", size: 3 }, { name: "Zed", kind: "dir", size: 0 }, { name: ".keep", kind: "file", size: 0 }, { name: "a.txt", kind: "file", size: 1 }, { name: "apps", kind: "dir", size: 0 }];
  ok(F.arrange(ents).map((e) => e.name).join(",") === "apps,Zed,a.txt,b.txt", "folders first, by name, marker hidden");
  ok(F.arrange(ents, " A").map((e) => e.name).join(",") === "apps,a.txt", "filter is case-blind substring");
  const big = Array.from({ length: 250 }, (_, i) => ({ name: "f" + i, kind: "file", size: i }));
  const p0 = F.pageSlice(big, 0), p2 = F.pageSlice(big, 2), pn = F.pageSlice(big, 99), pe = F.pageSlice([], 0);
  ok(p0.items.length === 100 && p0.pages === 3 && p0.from === 1 && p0.to === 100 && p2.items.length === 50 && p2.from === 201 && p2.to === 250 && pn.page === 2 && pe.pages === 1 && pe.from === 0 && pe.total === 0, "paging of large folders");
  const sp = F.spaces({ namespaces: [{ name: "team", role: "viewer", joined: true }, { name: "ops", role: "editor", joined: true }, { name: "x", role: null, joined: false }] });
  ok(sp.length === 7 && sp[0].id === "personal" && sp[1].id === "person:team" && sp[2].id === "shared:team", "spaces");
  ok(F.canWrite(sp[0]) && !F.canWrite(sp[1]) && !F.canWrite(sp[2]) && F.canWrite(sp[3]) && !F.canWrite(sp[5]) && !F.canWrite(null), "a viewer or an outsider is not offered writing");
  ok(F.query(sp[0], "") === "" && F.query(sp[0], "a b/c") === "?path=a%20b%2Fc" && F.query(sp[2], "x") === "?scope=shared&namespace=team&path=x", "query strings are encoded");
  ok(F.uploadCheck({ name: "a.bin", size: 5 }) === "" && F.uploadCheck({ name: "a/b", size: 5 }) !== "" && F.uploadCheck({ name: "big", size: F.MAX_UPLOAD + 1 }).includes("larger") && F.uploadCheck(null) !== "", "upload checks");
  ok(F.progressText(512, 1024).includes("50%") && F.progressText(0, 0) === "Starting ..." && F.progressText(2048, 1024).includes("100%"), "progress text");
  ok(F.canPreview({ kind: "file", size: 10 }) && !F.canPreview({ kind: "file", size: 600 * 1024 }) && !F.canPreview({ kind: "dir", size: 0 }), "preview only small text files");

  // ---- git
  ok(G.checkUrl("https://example.org/a/b.git") === "" && G.checkUrl("https://example.org:8443/a") === "", "good addresses");
  for (const u of ["", "http://example.org/a", "ssh://git@example.org/a", "https://user:pw@example.org/a", "https://localhost/a", "https://127.0.0.1/a", "https://10.0.0.5/a", "https://[::1]/a", "https://svc.cluster.local/a", "https://intranet/a", "https://x.internal/a", "https://exa mple.org/a", "git@example.org:a/b.git", "https://" + "a".repeat(600)]) {
    ok(G.checkUrl(u) !== "", "refused: " + u.slice(0, 40));
  }
  ok(G.defaultFolder("https://example.org/a/proj.git") === "proj" && G.defaultFolder("https://example.org/a/proj/") === "proj" && G.defaultFolder("https://example.org/") === "", "default folder");
  ok(G.checkFolder("proj-1_x") === "" && G.checkFolder("../x") !== "" && G.checkFolder("") !== "" && G.checkFolder("-x") !== "", "folder names");
  ok(G.checkBranch("") === "" && G.checkBranch("feature/x-1") === "" && G.checkBranch("--upload-pack=x") !== "" && G.checkBranch("a..b") !== "" && G.checkBranch("a b") !== "", "branch names (no option injection)");
  ok(G.parseDepth("").depth === 0 && G.parseDepth(" 5 ").depth === 5 && !G.parseDepth("-1").ok && !G.parseDepth("x").ok, "depth");
  ok(j(G.cloneBody(null, { url: " https://e.org/a.git ", folder: "a", branch: "", depth: 0 })) === '{"url":"https://e.org/a.git","folder":"a"}', "body without empty fields");
  ok(j(G.cloneBody({ scope: "shared", namespace: "t" }, { url: "https://e.org/a.git", folder: "a", branch: "main", depth: 1 })) === '{"url":"https://e.org/a.git","folder":"a","branch":"main","depth":1,"scope":"shared","namespace":"t"}', "body with scope");
  ok(G.cloneDone({ folder: "a", removed_links: 0 }) === "Cloned into a." && G.cloneDone({ folder: "a", removed_links: 2 }).includes("2 link"), "done text");
  ok(G.PLANNED === undefined, "nothing in the Git page is planned any more");
  ok(G.gitQuery(null, "p") === "?folder=p" && G.gitQuery({ scope: "shared", namespace: "t" }, "a b", { page: 2, per: 50, ref: "" }) === "?scope=shared&namespace=t&folder=a%20b&page=2&per=50", "git queries are encoded and leave out empty values");
  // a diff is drawn as text: markup in a repository stays text, and + and - colour the lines
  ok(G.diffClass({ kind: "add", text: "+x" }) === "diff-add" && G.diffClass({ kind: "del", text: "-x" }) === "diff-del" && G.diffClass({ kind: "hunk", text: "@@ -1 +1 @@" }) === "diff-hunk" && G.diffClass({ kind: "meta", text: "index 1..2" }) === "diff-meta" && G.diffClass({ kind: "ctx", text: "+not an add" }) === "diff-ctx", "the server's line kind decides the colour");
  ok(G.diffClass({ text: "+a" }) === "diff-add" && G.diffClass({ text: "-a" }) === "diff-del" && G.diffClass({ text: "+++ b/f" }) === "diff-ctx" && G.diffClass({ text: "--- a/f" }) === "diff-ctx" && G.diffClass({ text: "@@ x" }) === "diff-hunk" && G.diffClass({ text: " same" }) === "diff-ctx" && G.diffClass(null) === "diff-ctx", "a line without a kind is read by its first character");
  const evil = '+<img src=x onerror=alert(1)></script>"><b>x</b> password=hunter2';
  const rows = G.diffRows({ lines: [{ kind: "add", text: evil }, { kind: "del", text: "-y".repeat(3000) }] });
  ok(rows[0].text.startsWith("+<img src=x onerror=alert(1)>") && !rows[0].text.includes("hunter2") && rows[0].cls === "diff-add" && rows[1].text.length < 2100 && G.diffRows(null).length === 0, "diff text is kept as text, secrets hidden, long lines cut");
  ok(G.diffHeading({ new: "a.txt", old: "a.txt", status: "M", added: 2, removed: 1 }) === "a.txt  +2 -1" && G.diffHeading({ old: "a", new: "b", status: "R", added: 0, removed: 0 }).startsWith("a -> b") && G.diffHeading({ new: "p.bin", binary: true }).includes("binary"), "diff headings");
  ok(G.statusText({ branch: "main", ahead: 1, behind: 2, dirty: true, changed: 3, conflicted: 0, untracked: 1 }) === "main, 1 ahead, 2 behind, 3 changed, 1 new" && G.statusText({ branch: "main", dirty: false }) === "main, clean" && G.statusText({ detached: true, branch: "x" }).startsWith("detached") && G.statusText({ branch: "", dirty: false }).startsWith("no commits"), "status text");
  ok(G.canDiff({ kind: "changed" }) && G.canDiff({ kind: "deleted" }) && !G.canDiff({ kind: "new, not tracked" }) && !G.canDiff({ kind: "conflict" }) && !G.canDiff(null), "only a tracked change has a diff");
  ok(G.checkSha("abc1234") === "" && G.checkSha("zz;ls") !== "" && G.checkSha("") !== "" && G.checkSha("abc") !== "", "commit ids");
  ok(G.commitTitle({ short: "abc1234", subject: "Fix it" }) === "abc1234  Fix it" && G.changeLabel({ letter: "M", path: "a.txt", kind: "changed", staged: true, unstaged: false }).includes("staged"), "labels");

  // ---- files: rename and folder checks
  ok(F.renameCheck("a.txt", "b.txt", ["a.txt", "c.txt"]) === "" && F.renameCheck("a.txt", "a.txt", []) !== "" && F.renameCheck("a.txt", "c.txt", ["a.txt", "c.txt"]).includes("exists") && F.renameCheck("a", "x/y", []) !== "" && F.renameCheck("a", "..", []) !== "" && F.renameCheck("a", "", []) !== "" && F.renameCheck("a", " b", []) !== "" && F.renameCheck("a", F.MARKER, []) !== "", "rename checks");
  ok(F.moveTarget("d", "", "f.txt") === "d/f.txt" && F.moveTarget("d", "x/y", "f.txt") === "x/y/f.txt" && F.moveTarget("d", "../x", "f") === "" && F.moveTarget("d", "a/./b", "f") === "", "move targets stay below the space's root");
  ok(j(F.breadcrumbs("a/b c", "S").map((c) => c.path)) === '["","a","a/b c"]', "breadcrumbs keep spaces in names");

  // ---- flows
  ok(j(L.flowBadges({ production: true, published: true, triggers: 2 })) === '["production","published","2 triggers"]' && L.flowBadges({ triggers: 1 })[0] === "1 trigger" && L.flowBadges(null).length === 0, "flow badges");
  ok(L.describeFlow({ nodes: 1 }) === "1 step" && L.describeFlow({ nodes: 4 }) === "4 steps" && L.describeFlow({}) === "0 steps", "step count");
  ok(L.sortFlows([{ id: "2", name: "b" }, { id: "1", name: "A" }, { id: "3" }]).map((f) => f.id).join("") === "312" && L.sortFlows([{ id: "1", name: "Alpha" }, { id: "2", name: "beta" }], "ALP").length === 1, "sort and filter");
  ok(!L.definitionText({ node: { password: "hunter2" } }).includes("hunter2") && L.definitionText({ a: 1 }).includes('"a": 1') && L.definitionText({ s: "x".repeat(30000) }).length < 20100, "definition text hides secrets and is cut");

  ok(L.runState({ status: "blocked" }).text === "waiting for you" && L.runState({ status: "failed" }).cls === "err" && L.runState({ status: "succeeded" }).text === "done" && L.runState({}).text === "unknown" && L.runState({ status: "running" }).active && !L.runState({ status: "succeeded" }).active, "run states");
  ok(L.runLine({ id: "r1", status: "succeeded", done: 2, total: 3 }) === "r1, done, 2/3 steps" && L.runsQuery(2) === "?per=10&page=2" && L.runsQuery(-3) === "?per=10&page=0", "run lines and paging query");
  ok(L.runNote({ blocked: "delete the files? password=hunter2" }).includes("yourself") && !L.runNote({ blocked: "x password=hunter2" }).includes("hunter2") && L.runNote({ error: "boom" }) === "Error: boom" && L.runNote({}) === "", "a waiting question points to the person's own click; secrets hidden");
  ok(j(L.runBody(false)) === "{}" && j(L.runBody(true)) === '{"draft":true}' && L.PLANNED === undefined, "run body");

  // ---- runs
  ok(R.isActive("running") && !R.isActive("done") && !R.isActive(undefined), "active states");
  ok(R.checkPrompt("") !== "" && R.checkPrompt("   ") !== "" && R.checkPrompt("hi") === "" && R.checkPrompt("x".repeat(R.PROMPT_MAX + 1)) !== "", "prompt checks");
  ok(R.eventLine({ type: "delta", text: "x" }) === null && R.eventLine({ type: "thinking" }) === null && R.eventLine({ type: "timing" }) === null && R.eventLine(null) === null, "noise is not drawn");
  ok(R.eventLine({ type: "model", text: "hello", calls: [{ name: "files_read" }] }).text === "hello\nCalls: files_read", "model line");
  ok(R.eventLine({ type: "tool_result", name: "t", ok: false, text: "boom" }).kind === "failed" && R.eventLine({ type: "final", text: "done" }).kind === "final", "result kinds");
  ok(!R.eventLine({ type: "tool_result", name: "t", ok: true, text: "token=abc123secret" }).text.includes("abc123secret"), "secrets in tool output are hidden");
  ok(R.eventLine({ type: "tool_result", name: "t", ok: true, text: "y".repeat(5000) }).text.length < 2100, "long output is cut");
  const s1 = R.applyEvents({ lines: [], next: 0, status: "running" }, { events: [{ type: "model", text: "a" }, { type: "delta", text: "z" }], next: 2, status: "running", done: false });
  ok(s1.lines.length === 1 && s1.next === 2 && !s1.done, "events are folded in");
  const s2 = R.applyEvents(s1, { events: [{ type: "final", text: "b" }], next: 3, status: "done", done: true });
  ok(s2.lines.length === 2 && s2.done && s2.status === "done", "a finished run");
  ok(R.applyEvents(s2, { events: [], next: 1, status: "done" }).next === 3, "next never goes back");
  ok(R.applyEvents({ lines: [], next: 0, status: "running" }, { events: [], next: 0, status: "failed" }).done, "a status that is not active ends the polling");
  ok(j(R.verdictBody("approve")) === '{"verdict":"approve"}' && j(R.verdictBody("deny")) === '{"verdict":"deny"}', "the two verdicts");
  for (const w of ["always", "session", "approve_always", "", undefined, "APPROVE"]) { let threw = false; try { R.verdictBody(w); } catch (e) { threw = true; } ok(threw, "refused verdict: " + String(w)); }
  ok(R.approvalPath({ run: "r 1", call: "c/2" }) === "/api/v1/approvals/r%201/c%2F2", "approval path is encoded");
  ok(R.riskClass("High") === "err" && R.riskClass("low") === "muted", "risk colour");

  // ---- the shell: already signed in to the desktop
  const log = [];
  const mk = (status) => ({ storage: { getItem: () => null, setItem() {}, removeItem() {} }, fetch: async (url, init) => { log.push({ url, init }); if (status === "throw") throw new Error("x"); return { status, json: async () => ({ user: "root" }) }; } });
  const api200 = App.createApi(mk(200));
  ok(await api200.trySession() === true && api200.inSession() && log[0].url === "/api/v1/whoami" && log[0].init.credentials === "same-origin" && !log[0].init.headers.Authorization, "a desktop session skips the form (cookie sent to this origin only, no token invented)");
  await api200.get("/api/v1/version"); ok(log[1].init.credentials === "same-origin", "later calls ride the same session");
  const api401 = App.createApi(mk(401)); ok(await api401.trySession() === false && !api401.inSession(), "no desktop session shows the form");
  const apiOff = App.createApi(mk("throw")); ok(await apiOff.trySession() === false, "offline shows the form");
  const apiTok = App.createApi(mk(200)); await apiTok.login("a", "b"); ok(!apiTok.inSession(), "a form sign-in uses the token, not the cookie");
  const lg = [], st = { m: { [App.TOKEN_KEY]: "tk" }, getItem(k) { return this.m[k] || null; }, setItem(k, v) { this.m[k] = v; }, removeItem(k) { delete this.m[k]; } };
  const apiR = App.createApi({ storage: st, fetch: async (u, i) => { lg.push(i); return { status: 200, ok: true }; } });
  await apiR.raw("GET", "/api/v1/files/download?path=a");
  ok(lg[0].headers.Authorization === "Bearer tk" && lg[0].credentials === "omit" && apiR.authHeaders().Authorization === "Bearer tk", "raw calls (downloads, uploads) carry the same token");


  // ---- pages drawn against a fake page and fake API (states, and that an approval needs the person's click)
  function node(tag) {
    return { tag, attrs: {}, kids: [], _t: "", className: "", listeners: {}, value: "",
      setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.kids.push(c); return c; }, addEventListener(t, f) { this.listeners[t] = f; },
      set textContent(v) { this._t = v; this.kids = []; }, get textContent() { return this._t; } };
  }
  const doc = { createElement: node, createTextNode: (t) => ({ tag: "#text", _t: t, kids: [] }), body: node("body"), hidden: false };
  const walk = (n, f) => { f(n); for (const k of n.kids || []) walk(k, f); };
  const all = (n) => { const out = []; walk(n, (x) => out.push(x)); return out; };
  const texts = (n) => all(n).map((x) => x._t || "").join(" | ");
  const buttons = (n, label) => all(n).filter((x) => x.tag === "button" && x._t === label);
  const el = App.make(doc);
  function ctxFor(answers, calls, me) {
    return { el, doc, me: me || {}, say: (t, k) => { calls.say = [t, k]; }, poll: (f) => { calls.polls = (calls.polls || 0) + 1; }, messageFor: App.messageFor,
      api: { get: async (p) => { calls.push(["GET", p]); return answers["GET " + p.split("?")[0]] || { ok: false, status: 404, message: "Not found", data: null }; },
             post: async (p, b) => { calls.push(["POST", p, b]); return answers["POST " + p] || { ok: true, status: 200, data: {} }; }, authHeaders: () => ({}) } };
  }
  {
    const calls = [], host = node("div");
    await F.page().render(ctxFor({ "GET /api/v1/tenant": { ok: false, status: 403, message: "Not allowed: no access" } }, calls), host);
    ok(texts(host).includes("Not allowed: no access") && all(host).some((x) => x.attrs && x.attrs.role === "alert"), "Files: permission denied is shown, not a blank page");
  }
  {
    const calls = [], host = node("div");
    await F.page().render(ctxFor({ "GET /api/v1/tenant": { ok: true, status: 200, data: { namespaces: [] } }, "GET /api/v1/files": { ok: true, status: 200, data: { entries: [] } } }, calls), host);
    ok(texts(host).includes("This folder is empty.") && calls[1][1] === "/api/v1/files", "Files: empty state");
  }
  {
    const calls = [], host = node("div");
    const entries = Array.from({ length: 230 }, (_, i) => ({ name: "f" + String(i).padStart(3, "0"), kind: i % 10 === 0 ? "dir" : "file", size: i }));
    entries.push({ name: ".keep", kind: "file", size: 0 });
    await F.page().render(ctxFor({ "GET /api/v1/tenant": { ok: true, status: 200, data: { namespaces: [{ name: "t", role: "viewer", joined: true }] } }, "GET /api/v1/files": { ok: true, status: 200, data: { entries } } }, calls), host);
    ok(buttons(host, "Next").length === 1 && texts(host).includes("1 to 100 of 230") && !texts(host).includes(".keep"), "Files: a large folder is paged and the marker is hidden");
    ok(buttons(host, "Delete").length === 100 && buttons(host, "New folder").length === 1, "Files: own space offers writing");
  }
  {
    const calls = [], host = node("div");
    await G.page().render(ctxFor({ "GET /api/v1/tenant": { ok: true, status: 200, data: { namespaces: [] } }, "GET /api/v1/git/clones": { ok: true, status: 200, data: { clones: [{ folder: "p", branch: "main" }] } } }, calls), host);
    ok(texts(host).includes("p") && !texts(host).includes("Planned") && buttons(host, "Open").length === 1, "Git: clones listed with an Open button, no planned notice");
    const open = buttons(host, "Open")[0];
    const answers = {
      "GET /api/v1/git/status": { ok: true, status: 200, data: { branch: "main", ahead: 0, behind: 0, dirty: true, changed: 1, untracked: 0, conflicted: 0, origin: "x.example/o/r.git", changes: [{ path: "a.txt", from: "", kind: "changed", letter: "M", staged: false, unstaged: true }, { path: "n.txt", from: "", kind: "new, not tracked", letter: "?", staged: false, unstaged: true }], more: 0 } },
      "GET /api/v1/git/diff": { ok: true, status: 200, data: { files: [{ old: "a.txt", new: "a.txt", status: "M", added: 1, removed: 1, binary: false, lines: [{ kind: "hunk", text: "@@ -1 +1 @@" }, { kind: "del", text: "-<b>old</b>" }, { kind: "add", text: "+new" }] }] } },
    };
    const c2 = [], h2 = node("div");
    await G.page().render(ctxFor(Object.assign({ "GET /api/v1/tenant": { ok: true, status: 200, data: { namespaces: [] } }, "GET /api/v1/git/clones": { ok: true, status: 200, data: { clones: [{ folder: "p", branch: "main" }] } } }, answers), c2), h2);
    await buttons(h2, "Open")[0].listeners.click();
    ok(c2.some((c) => c[1] === "/api/v1/git/status?folder=p") && texts(h2).includes("main, 1 changed") && texts(h2).includes("M a.txt") && buttons(h2, "Diff").length === 1, "Git: status is read and only a tracked change has a Diff button");
    await buttons(h2, "Diff")[0].listeners.click();
    const lines = all(h2).filter((x) => x.className && x.className.startsWith("diff-line"));
    ok(c2.some((c) => c[1] === "/api/v1/git/diff?folder=p&path=a.txt") && lines.length === 3 && lines[1].className === "diff-line diff-del" && lines[1]._t === "-<b>old</b>" && lines[2].className === "diff-line diff-add", "Git: a diff is drawn as coloured text lines, markup stays text");
    const c3 = [], h3 = node("div");
    await G.page().render(ctxFor({ "GET /api/v1/tenant": { ok: true, status: 200, data: { namespaces: [] } }, "GET /api/v1/git/clones": { ok: true, status: 200, data: { clones: [{ folder: "p", branch: "main" }] } }, "GET /api/v1/git/status": { ok: false, status: 403, message: "Not allowed: no access" } }, c3), h3);
    await buttons(h3, "Open")[0].listeners.click();
    ok(texts(h3).includes("Not allowed: no access") && all(h3).some((x) => x.attrs && x.attrs.role === "alert"), "Git: a refused view says why");
    const c4 = [], h4 = node("div");
    await G.page().render(ctxFor({ "GET /api/v1/tenant": { ok: true, status: 200, data: { namespaces: [] } }, "GET /api/v1/git/clones": { ok: true, status: 200, data: { clones: [{ folder: "p", branch: "main" }] } }, "GET /api/v1/git/status": { ok: true, status: 200, data: { branch: "main", dirty: false, changes: [{ path: "p.bin", kind: "changed", letter: "M" }] } }, "GET /api/v1/git/diff": { ok: false, status: 415, message: "binary" } }, c4), h4);
    await buttons(h4, "Open")[0].listeners.click();
    await buttons(h4, "Diff")[0].listeners.click();
    ok(texts(h4).includes("That file is binary; a diff of it is not shown."), "Git: a binary file is refused in words");
  }
  {
    const calls = [], host = node("div");
    await L.page().render(ctxFor({ "GET /api/v1/flows": { ok: true, status: 200, data: { flows: [{ id: "f1", name: "Nightly", nodes: 3, triggers: 1, published: true }] } } }, calls), host);
    ok(texts(host).includes("Nightly") && texts(host).includes("3 steps") && !texts(host).includes("Planned") && buttons(host, "View").length === 1 && buttons(host, "Run").length === 1, "Flows: list with a Run button, no planned notice");
    const c3 = [], h3 = node("div");
    await L.page().render(ctxFor({ "GET /api/v1/flows": { ok: true, status: 200, data: { flows: [{ id: "f1", name: "Nightly", nodes: 3, published: true }] } }, "POST /api/v1/flows/f1/run": { ok: true, status: 200, data: { run: "r9", flow: "f1" } },
      "GET /api/v1/flows/f1/runs": { ok: true, status: 200, data: { runs: [{ id: "r9", status: "blocked", blocked: "delete the old files?", done: 1, total: 3 }, { id: "r8", status: "failed", error: "boom" }], more: true, page: 0, per: 10 } },
      "GET /api/v1/flows/f1": { ok: true, status: 200, data: { id: "f1" } } }, c3), h3);
    await buttons(h3, "Run")[0].listeners.click();
    const posts = c3.filter((c) => c[0] === "POST");
    ok(posts.length === 1 && posts[0][1] === "/api/v1/flows/f1/run" && j(posts[0][2]) === "{}", "Flows: one click on Run sends one run call (the published version)");
    ok(texts(h3).includes("r9, waiting for you, 1/3 steps") && texts(h3).includes("delete the old files?") && texts(h3).includes("Error: boom") && buttons(h3, "Older").length === 1 && all(h3).some((x) => x.tag === "a" && x.attrs.href === "#/runs"), "Flows: runs listed with the waiting question and a link to Runs");
    ok(!all(h3).some((x) => x.tag === "button" && /approve|deny|allow/i.test(x._t)) && c3.filter((c) => c[0] === "POST").length === 1, "Flows: no button answers a question");
    const c4 = [], h4 = node("div");
    await L.page().render(ctxFor({ "GET /api/v1/flows": { ok: true, status: 200, data: { flows: [{ id: "f1", name: "Nightly", nodes: 3, published: false }] } }, "POST /api/v1/flows/f1/run": { ok: false, status: 403, message: "Not allowed: no access" } }, c4), h4);
    await buttons(h4, "Run")[0].listeners.click();
    ok(j(c4.find((c) => c[0] === "POST")[2]) === '{"draft":true}', "Flows: a flow never published runs as a draft");
    const c2 = [], h2 = node("div");
    await L.page().render(ctxFor({ "GET /api/v1/flows": { ok: false, status: 403, message: "Not allowed: no access" } }, c2), h2);
    ok(texts(h2).includes("Not allowed"), "Flows: denied");
  }
  {
    const calls = [], host = node("div");
    const ap = [{ run: "r1", call: "c1", tool: "files_write", summary: "Write a.txt", detail: "password=hunter2", risk: "high" }];
    await R.page().render(ctxFor({ "GET /api/v1/chat/runs": { ok: true, status: 200, data: { runs: [] } }, "GET /api/v1/approvals": { ok: true, status: 200, data: { approvals: ap } } }, calls), host);
    ok(calls.polls === 1, "Runs: polls (no second protocol)");
    ok(texts(host).includes("Write a.txt") && !texts(host).includes("hunter2") && texts(host).includes("No runs yet"), "Runs: the question is shown, its secret is not");
    ok(calls.filter((c) => c[0] === "POST").length === 0, "Runs: nothing is approved or denied by itself");
    const [approve] = buttons(host, "Approve"), [deny] = buttons(host, "Deny");
    ok(!!approve && !!deny && !all(host).some((x) => x.tag === "button" && /always|session/i.test(x._t)), "Runs: Approve and Deny only, no 'always'");
    await approve.listeners.click();
    const posts = calls.filter((c) => c[0] === "POST");
    ok(posts.length === 1 && posts[0][1] === "/api/v1/approvals/r1/c1" && j(posts[0][2]) === '{"verdict":"approve"}', "Runs: one click sends one approve");
    ok(buttons(host, "Approve").length === 0, "Runs: the answered question is gone and cannot be answered twice");
  }
  {
    const calls = [], host = node("div");
    await R.page().render(ctxFor({ "GET /api/v1/chat/runs": { ok: false, status: 403, message: "Not allowed: no chat permission" }, "GET /api/v1/approvals": { ok: false, status: 403, message: "x" } }, calls), host);
    ok(texts(host).includes("Not allowed: no chat permission"), "Runs: denied is shown");
  }

  console.log(bad ? "FAILED " + bad : "ALL OK");
  process.exit(bad ? 1 : 0);
})();
"##;

#[test]
fn the_web_ui_page_logic_follows_its_rules_under_node() {
    let ok = std::process::Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: node is not installed");
        return;
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webservice/ui");
    let dir = std::env::temp_dir().join(format!("iyakki-webui-pages-js-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["app.js", "common.js", "files.js", "git.js", "flows.js", "runs.js"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::write(dir.join("driver.js"), DRIVER).unwrap();
    let out = std::process::Command::new("node").arg(dir.join("driver.js")).arg(&dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success() && !text.contains("BAD"), "{text}");
    assert!(text.contains("ALL OK"), "{text}");
}
