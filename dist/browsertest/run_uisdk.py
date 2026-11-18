#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).
"""Real-browser test of the UI SDK's browser renderer (F-SDK-20) and of the client WASM engine drawing the same list (F-SDK-22). Needs Chrome and the built `iyakki-ui-preview` program.

  python3 dist/browsertest/run_uisdk.py --preview-bin PATH [--out DIR]

Build the program with: cargo build -p iyakki-ui-preview. Exit codes as run_pages.py (0 passed, 1 failed, 2 nothing was tested).
"""
import argparse
import datetime
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cdp  # noqa: E402
import harness  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    home_work = os.environ.get("IYAKKI_WORK", os.path.expanduser("~/iyakki-work"))
    ap.add_argument("--out", default=os.path.join(home_work, "browsertest", "uisdk-" + datetime.date.today().isoformat()))
    ap.add_argument("--work", default=os.path.join(home_work, "browsertest", "work"))
    ap.add_argument("--preview-bin", required=True)
    a = ap.parse_args()
    cdp.install_signal_handlers()
    binary = cdp.find_chrome()
    if not binary or cdp.chrome_major(binary) < cdp.MIN_MAJOR:
        print("NOTHING WAS TESTED: no Chrome or Chromium of version %d or newer." % cdp.MIN_MAJOR)
        return 2
    if not os.path.isfile(a.preview_bin):
        print("NOTHING WAS TESTED: %s does not exist." % a.preview_bin)
        return 2
    rep = harness.Report(a.out)
    chrome = cdp.Chrome(a.work, binary=binary, hard_limit=300).start()
    try:
        rep.info("browser", chrome.version()["product"])
        import uisdksuite
        rep.run("uisdk", uisdksuite.suite, chrome, a.preview_bin)
        import uiengsuite
        rep.run("uiengine", uiengsuite.suite, chrome, a.preview_bin, os.path.abspath(os.path.join(HERE, "..", "..")))
    finally:
        chrome.stop()
    return 1 if rep.summary() else 0


if __name__ == "__main__":
    sys.exit(main())
