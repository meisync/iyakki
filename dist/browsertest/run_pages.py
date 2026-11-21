#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Real-browser tests of the web UI, the tile renderer and the client engine. Needs Chrome and a built iyakki-agent test binary.

  python3 dist/browsertest/run_pages.py [--out DIR] [--only pages,jsdrivers,tiles,client] [--build]

Exit codes: 0 every check passed (known issues are listed, not failures), 1 a check failed, 2 NOTHING WAS TESTED (no Chrome or
Chromium found or too old, no cargo or the test binaries cannot be built). See dist/browsertest/README.md.

Everything runs on loopback only. Chrome uses a throw-away profile under the work folder and is always killed at the end.
Screenshots and logs go to --out (default ~/iyakki-work/browsertest/<date>), outside the repository.
"""
import argparse
import datetime
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cdp  # noqa: E402
import harness  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(HERE))
UI = os.path.join(ROOT, "crates/iyakki-agent/src/webservice/ui")


def build(target_dir, log):
    env = dict(os.environ, CARGO_TARGET_DIR=target_dir)
    outs = {}
    for args in (["-p", "iyakki-agent", "--lib"], ["-p", "iyakki-agent", "--test", "webui_tiles_js"]):
        r = subprocess.run(["cargo", "test", "--no-run", "--locked"] + args, cwd=ROOT, env=env, capture_output=True, text=True)
        log.write(r.stdout + r.stderr)
        if r.returncode:
            raise SystemExit("cargo build failed; see the log")
        outs.update(harness.cargo_executables(r.stdout + r.stderr))
    return outs


def main():
    ap = argparse.ArgumentParser()
    home_work = os.environ.get("IYAKKI_WORK", os.path.expanduser("~/iyakki-work"))
    ap.add_argument("--out", default=os.environ.get("IYAKKI_BROWSERTEST_OUT") or os.path.join(home_work, "browsertest", datetime.date.today().isoformat()))
    ap.add_argument("--work", default=os.environ.get("IYAKKI_BROWSERTEST_WORK") or os.path.join(home_work, "browsertest", "work"), help="throw-away Chrome profiles and fixture files")
    ap.add_argument("--target-dir", default=os.environ.get("CARGO_TARGET_DIR") or os.path.join(home_work, "browsertest-target"), help="cargo target folder for the two test binaries")
    ap.add_argument("--agent-bin", help="a built iyakki-agent lib test binary (skips cargo)")
    ap.add_argument("--tiles-bin", help="a built webui_tiles_js test binary (skips cargo)")
    ap.add_argument("--only", default="pages,functions,more,jsdrivers,tiles,client")
    ap.add_argument("--hard-limit", type=int, default=1500)
    a = ap.parse_args()
    only = set(a.only.split(","))
    cdp.install_signal_handlers()
    binary = cdp.find_chrome()
    if not binary:
        print("NOTHING WAS TESTED: no Chrome or Chromium found (set IYAKKI_CHROME to its path; looked in the usual places and on PATH).")
        return 2
    major = cdp.chrome_major(binary)
    if major < cdp.MIN_MAJOR:
        print("NOTHING WAS TESTED: %s is version %s; version %d or newer is needed." % (binary, major or "unknown", cdp.MIN_MAJOR))
        return 2
    os.makedirs(a.out, exist_ok=True)
    rep = harness.Report(a.out)
    if a.agent_bin and a.tiles_bin:
        lib, tiles_bin = a.agent_bin, a.tiles_bin
    else:
        if not shutil.which("cargo"):
            print("NOTHING WAS TESTED: cargo is not on PATH and no --agent-bin and --tiles-bin were given.")
            return 2
        with open(os.path.join(a.out, "build.log"), "w") as log:
            try:
                exes = build(a.target_dir, log)
            except SystemExit as e:
                print("NOTHING WAS TESTED: %s (see %s)" % (e, os.path.join(a.out, "build.log")))
                return 2
        lib = exes.get("src/lib.rs")
        tiles_bin = exes.get("tests/webui_tiles_js.rs")
    if not (lib and tiles_bin and os.path.isfile(lib) and os.path.isfile(tiles_bin)):
        print("NOTHING WAS TESTED: the test binaries were not found.")
        return 2
    work = a.work
    chrome = cdp.Chrome(work, binary=binary, hard_limit=a.hard_limit).start()
    try:
        rep.info("browser", chrome.version()["product"])
        if "pages" in only:
            import pagesuite
            for prof, tag in (("cloud", "cloud"), ("agent-api", "agent")):
                with harness.Fixture(lib, work, prof) as fx:
                    rep.run("pages-" + tag, pagesuite.suite_pages, chrome, fx, tag)
        if "functions" in only:
            import funcsuite
            with harness.Fixture(lib, work, "cloud") as fx:
                rep.run("functions-cloud", funcsuite.suite, chrome, fx, work)
        if "more" in only:
            import morepages
            with harness.Fixture(lib, work, "agent-api") as fx:
                rep.run("more-agent", morepages.suite_agent, chrome, fx)
            with harness.Fixture(lib, work, "cloud", env={"IYAKKI_BROWSERTEST_TENANTS": "2"}) as fx:
                rep.run("more-cloud", morepages.suite_cloud, chrome, fx)
        if "jsdrivers" in only:
            import jsdrivers
            rep.run("jsdrivers", jsdrivers.suite, chrome, UI, ROOT, tiles_bin, work)
        if "tiles" in only:
            import tilesuite
            rep.run("tiles", tilesuite.suite, chrome, UI, tiles_bin, work)
            with harness.Fixture(lib, work, "agent-api") as fx:
                rep.run("tiles-service", tilesuite.service, chrome, fx, work)
        if "client" in only:
            import clientsuite
            rep.run("client", clientsuite.suite, chrome, ROOT)
            rep.run("client-page", clientsuite.page_suite, chrome, ROOT)
            rep.run("client-media", clientsuite.media_suite, chrome, ROOT)
            rep.run("client-app", clientsuite.app_suite, chrome, ROOT)
    finally:
        chrome.stop()
    return 1 if rep.summary() else 0


if __name__ == "__main__":
    sys.exit(main())
