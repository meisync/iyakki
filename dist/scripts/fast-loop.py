#!/usr/bin/env python3
"""The fast loop: edit -> test the change -> running on a device (docs/development/FAST-LOOP.md).

  ./run.sh fast [--device NAME] [--base REV] [--no-test] [--no-push] [--dry] [--profile device] [--against OLDFILE] [--home DIR] [--apply] [--fetch CMD] [--serial]

Steps, each timed, stopping at the first failure:
  1 detect  which files changed (git, like test-affected.py) and which LAYER of the device they belong to:
              live-ui      only ui/apps crates changed: the new program takes effect without a device restart (tile process restart or core re-exec)
              live-core    core/base crates changed (the program of PID 1): live by the in-place re-exec of the core
              reboot       the DEVICE layer changed (kernel, modules, boot files, first stage, image recipe): the staged update plus a restart
              nothing      docs and scripts only
            The verdict is printed BEFORE anything is built or sent. A `reboot` verdict stops the loop (use dist/buildroot/quick-push.sh or nightly-push.sh,
            then `iyakki dev update` and `iyakki dev reboot`); `--force-program` pushes the program anyway.
  2 build   `cargo build --profile device --target x86_64-unknown-linux-musl -p iyakki-core --bin iyakki` (incremental, no LTO), only when code changed
  3 test    dist/scripts/test-affected.py --quick --run (the changed crates' own tests), run beside the build (--serial: after it); a red test stops the push
  4 push    `iyakki dev push-apps --bin FILE --layer VERDICT` to the chosen device (the approval card is answered on the device; nothing is rebooted)
  5 apply   (--apply) `iyakki dev shell -- iyakki live-restart PROGRAM`: the running program replaces itself in place (docs/development/LIVE-RESTART.md; the device
            needs /etc/iyakki/live-restart.enabled and a program that has the wiring; the person on the device answers the shell approval)
  --fetch CMD  the build ran on another machine (a build host behind a `cargo` wrapper): CMD copies the finished program to $FAST_BIN on this one
Run it under the one-cargo rule of the shared machines, for example
  flock ~/iyakki-work/.cargo.lock env CARGO_TARGET_DIR=... ./run.sh fast --no-push
"""
import fnmatch
import importlib.util
import os
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True, cwd=HERE).strip()

# Paths that belong to the DEVICE layer: the kernel, its modules, the boot files, the first stage and the image recipe. A change here needs the staged
# update and a restart; everything else is the program (crates) or text.
DEVICE_LAYER = [
    "dist/buildroot/*.fragment", "dist/buildroot/*defconfig*", "dist/buildroot/board/*", "dist/buildroot/kernel*", "dist/buildroot/configs/*",
    "dist/buildroot/package/*", "dist/buildroot/initramfs*", "dist/buildroot/grub*", "dist/buildroot/layers.txt",
    "iyakki-boot/*", "crates/iyakki-stage1/*", "crates/iyakki-fw/*",
]


def load_affected():
    spec = importlib.util.spec_from_file_location("test_affected", os.path.join(HERE, "test-affected.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def decide(files, plan):
    """(verdict, reasons) for a change: nothing | live-ui | live-core | reboot. `plan` is test-affected's plan() result (changed crates, layers)."""
    dev = [f for f in files if any(fnmatch.fnmatch(f, g) for g in DEVICE_LAYER)]
    if dev:
        return "reboot", ["device layer: " + ", ".join(dev[:4]) + (" ..." if len(dev) > 4 else "")]
    if plan["root"]:
        return "live-core", ["a root file (Cargo.toml/Cargo.lock) changed: the whole program is rebuilt"]
    changed = plan["changed"]
    if not changed:
        return "nothing", ["no program code changed"]
    core = [c for c in changed if plan["layers"].get(c) in ("base", "core", "agent")]
    if core:
        return "live-core", ["core/base crates: " + ", ".join(core)]
    return "live-ui", ["ui/apps crates: " + ", ".join(changed)]


# where `push-apps` leaves the new program on the device (the patch layer on the data volume); the running program is /usr/bin/iyakki
LIVE_PROGRAM = "/var/lib/iyakki/iyakki/patches/push-apps/layer/usr/bin/iyakki"


def apply_argv(binary, opts, program=LIVE_PROGRAM):
    """The command that makes the running program replace itself with the pushed one (no device restart): `iyakki dev shell -- iyakki live-restart PROGRAM`."""
    cmd = [binary, "dev"]
    for k in ("--device", "--home"):
        if opts.get(k):
            cmd += [k, opts[k]]
    return cmd + ["shell", "--minutes", "2", "--reason", "live restart of the program just pushed", "--", "/usr/bin/iyakki", "live-restart", program]


class Steps:
    def __init__(self):
        self.rows = []
        self.t0 = time.monotonic()

    def run(self, name, argv, env=None):
        print(f"\n[fast] {name}: {' '.join(argv)}", flush=True)
        t = time.monotonic()
        rc = subprocess.call(argv, cwd=ROOT, env=env)
        self.rows.append((name, time.monotonic() - t, rc))
        return rc

    def start(self, name, argv, env=None):
        """Run a step beside the others (its output is kept and printed when it is joined)."""
        print(f"\n[fast] {name} (alongside the build): {' '.join(argv)}", flush=True)
        f = tempfile.TemporaryFile("w+")
        return (name, subprocess.Popen(argv, cwd=ROOT, env=env, stdout=f, stderr=subprocess.STDOUT, text=True), f, time.monotonic())

    def join(self, h):
        name, p, f, t0 = h
        rc = p.wait()
        secs = time.monotonic() - t0  # from its start, so the rows overlap
        f.seek(0)
        sys.stdout.write(f"\n[fast] ---- output of {name} ----\n" + f.read())
        f.close()
        self.rows.append((name + "*", secs, rc))
        self.alongside = True
        return rc

    def note(self, name, secs, rc=0):
        self.rows.append((name, secs, rc))

    def table(self):
        print("\n[fast] step        time      result")
        for n, s, rc in self.rows:
            print(f"[fast] {n:<10} {s:6.1f} s  {'ok' if rc == 0 else 'FAILED (' + str(rc) + ')'}")
        wall = time.monotonic() - self.t0
        print(f"[fast] total      {wall:6.1f} s wall" + ("  (* ran alongside the build, so the rows add up to more than the total)" if getattr(self, "alongside", False) else ""))


def main(argv):
    opts = {"--base": "HEAD~1", "--device": None, "--profile": "device", "--against": None, "--home": None, "--fetch": None}
    flags = set()
    a = list(argv)
    while a:
        x = a.pop(0)
        if x in opts:
            opts[x] = a.pop(0)
        elif x in ("--no-test", "--no-push", "--dry", "--force-program", "--apply", "--serial"):
            flags.add(x)
        else:
            print(__doc__)
            return 2
    st = Steps()
    t = time.monotonic()
    ta = load_affected()
    names, by_dir, deps = ta.workspace()
    files = ta.changed_files(opts["--base"])
    plan = ta.plan(files, names, by_dir, deps, ta.load_layers(os.path.join(HERE, "crate-layers.txt")))
    verdict, why = decide(files, plan)
    st.note("detect", time.monotonic() - t)
    print(f"[fast] {len(files)} changed file(s); verdict: {verdict}  ({'; '.join(why)})")
    msg = {
        "nothing": "nothing to build or push.",
        "live-ui": "LIVE: the apps/ui program takes effect without a device restart (once live apply is on for the device).",
        "live-core": "LIVE: the core program is swapped in place (re-exec of PID 1, docs/development/LIVE-RESTART.md; off by default).",
        "reboot": "REBOOT NEEDED (kernel/base/boot files changed): use dist/buildroot/quick-push.sh or nightly-push.sh, `iyakki dev update`, `iyakki dev reboot`.",
    }[verdict]
    print("[fast] " + msg)
    if verdict == "nothing" or "--dry" in flags:
        st.table()
        return 0
    if verdict == "reboot" and "--force-program" not in flags:
        st.table()
        return 3
    target = "x86_64-unknown-linux-musl"
    # the tests of the change and the build of the program do not need each other: run them side by side (--serial: one after the other)
    test_argv = [sys.executable, os.path.join(HERE, "test-affected.py"), "--quick", "--run", "--base", opts["--base"]]
    side = None
    if "--no-test" not in flags and "--serial" not in flags:
        side = st.start("test", test_argv)
    rc = st.run("build", ["cargo", "build", "--profile", opts["--profile"], "--target", target, "-p", "iyakki-core", "--bin", "iyakki"])
    if rc:
        if side:
            st.join(side)
        st.table()
        return rc
    target_dir = os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target"))
    binary = os.path.join(target_dir, target, opts["--profile"], "iyakki")
    if opts["--fetch"]:
        os.makedirs(os.path.dirname(binary), exist_ok=True)
        rc = st.run("fetch", ["sh", "-c", opts["--fetch"]], env=dict(os.environ, FAST_BIN=binary))
        if rc:
            if side:
                st.join(side)
            st.table()
            return rc
    if side:
        rc = st.join(side)
        if rc:
            st.table()
            return rc
    elif "--no-test" not in flags:
        rc = st.run("test", test_argv)
        if rc:
            st.table()
            return rc
    if "--no-push" not in flags:
        cmd = [binary, "dev", "push-apps", "--bin", binary, "--layer", verdict]
        for k in ("--device", "--against", "--home"):
            if opts[k]:
                cmd += [k, opts[k]]
        rc = st.run("push", cmd)
        if rc == 0 and "--apply" in flags:
            rc = st.run("apply", apply_argv(binary, opts))
    st.table()
    return rc if "--no-push" not in flags else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
