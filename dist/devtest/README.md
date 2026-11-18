# dist/devtest: device UI test runner

Drives a paired IYAKKI device (or a QEMU guest) through developer mode, takes screenshots, checks them and writes a report.
Full guide: `docs/development/DEVICE-UI-TESTS.md`.

```
./run.sh device-ui --list
./run.sh device-ui --device lenovo-t7 --scenarios all --out ~/iyakki-work/devtest-runs/RUN
./run.sh device-ui --device lenovo-t7 --scenarios home-launcher,display-theme --out DIR
./run.sh device-ui --qemu target/buildroot-pc-push --out DIR        # boots a scratch guest (needs tesseract to pair it)
python3 -m unittest discover dist/devtest/tests                      # no device needed
```

Output: `DIR/<device>/<scenario>/NN-label.png`, `DIR/<device>/results.json`, `DIR/<device>/report.html` (self-contained).
Exit status: 0 all run scenarios passed (skips are fine), 1 a scenario failed or errored, 2 nothing could run.

Files: `devui.py` (Device, guard, state/TSV parsing, contrast maths, report), `builtin_scenarios.py` (the Python scenarios and the
`.scn` reader), `scenarios/*.scn` (one-step-per-line scenarios), `qemu_guest.py` (the scratch guest), `run.py` (the runner), `tests/`.
Python 3 standard library only; Pillow and tesseract are used when installed and their checks are reported as skipped when not.
