# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Runs the JavaScript drivers of `crates/iyakki-agent/tests/webui_*_js.rs` in real Chrome instead of node.

The drivers are taken from the Rust files as they are (the text of each `r##"..."##` constant): nothing is copied, so the node test and
this run cannot drift apart. Only the node parts they use are stood in for inside the page: `require` (the UI files by name, the `path`
and `fs` modules), `process` (argv and exit), `console`, `global` and `setImmediate`. Each driver gets a fresh page.
"""
import json
import os
import re
import subprocess

import harness

RUNNER = r"""<!doctype html><meta charset="utf-8"><title>runner</title><body><script>
async function runDriver(src, modNames, fileNames) {
  const lines = []; let code = null;
  const modules = {}, sources = {};
  async function load(n) {
    if (sources[n] !== undefined) return;
    const r = await fetch('/ui/' + n); if (!r.ok) throw new Error('no ' + n);
    sources[n] = await r.text();
    for (const m of sources[n].matchAll(/require\("\.\/([\w.]+\.js)"\)/g)) await load(m[1]);
  }
  for (const n of modNames) await load(n);
  const files = {};
  for (const f of fileNames) { const r = await fetch('/golden/' + f); files[f] = new Uint8Array(await r.arrayBuffer()); }
  function req(name) {
    const base = String(name).split('/').pop();
    if (name === 'path') return { join: (...a) => a.join('/') };
    if (name === 'fs') return { readFileSync: (p, enc) => { const k = String(p).split('/').pop(); if (!(k in files)) throw new Error('ENOENT ' + p); return enc ? new TextDecoder().decode(files[k]) : files[k]; } };
    if (modules[base]) return modules[base].exports;
    if (sources[base] === undefined) throw new Error('module not preloaded: ' + name);
    const m = { exports: {} }; modules[base] = m;
    new Function('module', 'exports', 'require', sources[base])(m, m.exports, req);
    return m.exports;
  }
  const con = { log: (...a) => lines.push(a.join(' ')), error: (...a) => lines.push('ERR ' + a.join(' ')), warn: (...a) => lines.push('WARN ' + a.join(' ')) };
  const proc = { argv: ['node', 'driver.js', '/d'], exit: (c) => { if (code === null) code = c; } };
  try { new Function('require', 'process', 'console', 'global', 'setImmediate', src)(req, proc, con, window, (f) => setTimeout(f, 0)); }
  catch (e) { lines.push('BAD sync exception ' + e.stack); if (code === null) code = 1; }
  const t0 = Date.now();
  while (code === null && Date.now() - t0 < 60000) await new Promise((r) => setTimeout(r, 25));
  if (code === null) { lines.push('BAD the driver never finished'); code = 1; }
  return { code, lines };
}
</script>"""


def extract(rust_path):
    """{constant name: JS text} for every `const NAME: &str = r##"..."##;` of a Rust file."""
    txt = open(rust_path).read()
    return {m.group(1): m.group(2) for m in re.finditer(r'const (\w+): &str = r##"(.*?)"##;', txt, re.S)}


DRIVERS = [
    ("webui_js.rs", "DRIVER", "app, cloud and remote: API client, errors, page choice, role rules"),
    ("webui_pages_js.rs", "DRIVER", "common, files, git, flows, runs page logic and states"),
    ("webui_schedules_js.rs", "DRIVER", "schedules page logic and states"),
    ("webui_tenant_js.rs", "DRIVER", "tenant control"),
    ("webui_tenantcfg_js.rs", "DRIVER", "tenant settings page"),
    ("webui_notifications_js.rs", "DRIVER", "notifications page and the bell"),
    ("webui_tiles_js.rs", "GOLDEN", "tile renderer against the Rust reference pixels (canvas stand-in, enlargement, hostile input, hit lookup)"),
    ("webui_tiles_js.rs", "PAGE", "tiles page controller"),
]


def write_goldens(tiles_bin, gdir):
    env = dict(os.environ, IYAKKI_BROWSERTEST_GOLDEN=gdir)
    r = subprocess.run([tiles_bin, "--ignored", "--exact", "write_browser_goldens", "--nocapture"], env=env, capture_output=True, text=True, timeout=120)
    if r.returncode != 0 or not os.path.exists(os.path.join(gdir, "cases.txt")):
        raise RuntimeError("golden writer failed: " + r.stdout[-500:] + r.stderr[-500:])
    return open(os.path.join(gdir, "cases.txt")).read().split("\n")


def suite(rep, chrome, ui_dir, root, tiles_bin, work):
    tests = os.path.join(root, "crates/iyakki-agent/tests")
    gdir = os.path.join(work, "golden")
    os.makedirs(gdir, exist_ok=True)
    cases = write_goldens(tiles_bin, gdir)
    gfiles = ["cases.txt"] + [c + ext for c in cases for ext in (".json", ".rgba")]
    srv = harness.Static({"/ui/": ui_dir, "/golden/": gdir}, {"/runner.html": ("text/html", RUNNER.encode())})
    page = chrome.new_page(1000, 700)
    try:
        for fname, const, what in DRIVERS:
            src = extract(os.path.join(tests, fname))[const]
            mods = sorted(set(re.findall(r'"(\w+\.js)"', src)))
            want_files = gfiles if "readFileSync" in src else []
            page.goto(srv.base + "/runner.html")
            page.clear_errors()
            r = page.evaluate("runDriver(%s, %s, %s)" % (json.dumps(src), json.dumps(mods), json.dumps(want_files)), timeout=120)
            lines = r["lines"]
            oks = [l for l in lines if l.startswith("OK")]
            bad = [l for l in lines if l.startswith("BAD") or l.startswith("ERR")]
            name = "%s %s (%s)" % (fname, const, what)
            with open(rep.shot("jsdriver-%s-%s.log" % (fname.replace(".rs", ""), const)), "w") as f:
                f.write("\n".join(lines))
            rep.check(name + ": exit code 0, no BAD line, ALL OK printed (%d OK lines)" % len(oks), r["code"] == 0 and not bad and any("ALL OK" in l for l in lines), "\n".join(bad[:8] + lines[-3:]))
    finally:
        page.close()
        srv.stop()
