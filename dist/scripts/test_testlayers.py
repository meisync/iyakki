#!/usr/bin/env python3
"""Tests of the stamp logic and the layer runner on FAKE trees: a throw-away git repository with a few made-up crates and a fake `cargo`
(a shell script), so no real cargo runs. Run: python3 -I dist/scripts/test_testlayers.py   (the Rust test layer_split runs it too)."""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

os.environ.pop("IYAKKI_ROOT", None)  # the Rust test layer_split sets it for the real tree; these tests use fake trees only
HERE = os.path.dirname(os.path.abspath(__file__))
REAL = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
import testlayers as tl  # noqa: E402

LAYERS = "iyakki-lib base\niyakki-core core\niyakki-shell ui\niyakki-tile-* apps\n"
CARGO = '#!/bin/sh\necho "$@" >> "$FAKE_CARGO_LOG"\ncase "$*" in *--exact*) exit 0 ;; esac\nif [ -f "$FAKE_CARGO_FAIL" ]; then echo "test never_passes ... FAILED"; echo "failures:"; echo "    never_passes"; exit 101; fi\necho "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"\nexit 0\n'


def slurp(path):
    with open(path) as f:
        return f.read()


def sh(root, *a):
    subprocess.check_call(["git", "-C", root] + list(a), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def write(root, rel, text):
    p = os.path.join(root, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w") as f:
        f.write(text)


def commit(root, msg="x"):
    sh(root, "add", "-A")
    sh(root, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", msg)


def fake_repo(extra_crates=()):
    root = tempfile.mkdtemp(prefix="iyakki-fakelayers-")
    sh(root, "init", "-q")
    names = ["iyakki-lib", "iyakki-core", "iyakki-shell", "iyakki-tile-one", "iyakki-tile-two"] + list(extra_crates)
    write(root, "Cargo.toml", "[workspace]\nmembers = [" + ", ".join(f'"crates/{n}"' for n in names) + "]\n")
    write(root, "Cargo.lock", "# lock 1\n")
    for n in names:
        write(root, f"crates/{n}/Cargo.toml", f'[package]\nname = "{n}"\nversion = "0.1.0"\n')
        write(root, f"crates/{n}/src/lib.rs", f"// {n}\n")
    write(root, "dist/scripts/crate-layers.txt", LAYERS)
    write(root, "FEATURES.md", "features 1\n")
    write(root, "docs/a.md", "doc 1\n")
    write(root, "site/index.html", "site 1\n")
    commit(root, "init")
    return root


class Logic(unittest.TestCase):
    def setUp(self):
        self.root = fake_repo()
        self.stamps = tempfile.mkdtemp(prefix="iyakki-fakestamps-")
        os.environ["IYAKKI_STAMP_DIR"] = self.stamps
        self.rules = tl.load_rules(os.path.join(self.root, "dist/scripts/crate-layers.txt"))
        self.crates = tl.workspace_crates(self.root)

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)
        shutil.rmtree(self.stamps, ignore_errors=True)
        os.environ.pop("IYAKKI_STAMP_DIR", None)

    def h(self, group):
        self.crates = tl.workspace_crates(self.root)
        return tl.input_hash(self.root, group, self.crates, self.rules)

    def test_groups_are_the_whole_workspace(self):
        g = tl.split_groups(self.crates, self.rules)
        self.assertEqual(g["core"], ["iyakki-core", "iyakki-lib"])
        self.assertEqual(g["apps"], ["iyakki-shell", "iyakki-tile-one", "iyakki-tile-two"])
        self.assertEqual(sorted(sum(g.values(), [])), sorted(self.crates))

    def test_a_crate_without_a_layer_fails(self):
        r = fake_repo(["iyakki-newcomer"])
        try:
            with self.assertRaises(tl.LayerError) as e:
                tl.split_groups(tl.workspace_crates(r), self.rules)
            self.assertIn("iyakki-newcomer", str(e.exception))
            env = dict(os.environ, IYAKKI_ROOT=r)
            p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "testlayers.py"), "check"], env=env, capture_output=True, text=True)
            self.assertEqual(p.returncode, 1)
            self.assertIn("iyakki-newcomer", p.stderr)
        finally:
            shutil.rmtree(r, ignore_errors=True)

    def test_unknown_layer_name_fails(self):
        with self.assertRaises(tl.LayerError):
            tl.split_groups(self.crates, [("sideways", "iyakki-*")])

    def test_hash_is_stable_and_ignores_mtimes(self):
        a, _ = self.h("core")
        os.utime(os.path.join(self.root, "crates/iyakki-lib/src/lib.rs"), (1, 1))
        b, _ = self.h("core")
        self.assertEqual(a, b)
        self.assertEqual(len(a), 64)

    def test_apps_change_moves_only_the_apps_hash(self):
        c0, _ = self.h("core"); a0, _ = self.h("apps")
        write(self.root, "crates/iyakki-tile-one/src/lib.rs", "// changed\n"); commit(self.root)
        c1, _ = self.h("core"); a1, _ = self.h("apps")
        self.assertEqual(c0, c1)
        self.assertNotEqual(a0, a1)

    def test_strict_mode_follows_dependencies_upwards_and_the_plain_hash_does_not(self):
        # iyakki-core depends on the apps crate (as in the real workspace): plain keeps its stamp when the apps change, strict does not
        write(self.root, "crates/iyakki-core/Cargo.toml", '[package]\nname = "iyakki-core"\nversion = "0.1.0"\n[dependencies]\niyakki-tile-one = { path = "../iyakki-tile-one" }\n'); commit(self.root)
        plain0 = tl.input_hash(self.root, "core", self.crates, self.rules, False)[0]
        strict0 = tl.input_hash(self.root, "core", self.crates, self.rules, True)[0]
        self.assertNotEqual(plain0, strict0)
        self.assertEqual(tl.workspace_deps(self.root, tl.workspace_crates(self.root))["iyakki-core"], ["iyakki-tile-one"])
        write(self.root, "crates/iyakki-tile-one/src/lib.rs", "// changed\n"); commit(self.root)
        self.assertEqual(plain0, tl.input_hash(self.root, "core", self.crates, self.rules, False)[0])
        self.assertNotEqual(strict0, tl.input_hash(self.root, "core", self.crates, self.rules, True)[0])
        # the stamp holds both; the release check (strict) refuses a stamp written before the apps change, the loop (plain) accepts it
        h, _ = (plain0, None)
        os.environ.pop("IYAKKI_STAMP_STRICT", None)
        sh(self.root, "reset", "-q", "--hard", "HEAD~1")
        tl.write_stamp(self.root, "core", tl.workspace_crates(self.root), self.rules, plain0)
        write(self.root, "crates/iyakki-tile-one/src/lib.rs", "// changed\n"); commit(self.root)
        self.assertTrue(tl.verify(self.root, "core", self.crates, self.rules)[0])
        os.environ["IYAKKI_STAMP_STRICT"] = "1"
        try:
            self.assertFalse(tl.verify(self.root, "core", self.crates, self.rules)[0])
        finally:
            os.environ.pop("IYAKKI_STAMP_STRICT", None)

    def test_core_change_moves_both(self):
        c0, _ = self.h("core"); a0, _ = self.h("apps")
        write(self.root, "crates/iyakki-lib/src/lib.rs", "// changed\n"); commit(self.root)
        c1, _ = self.h("core"); a1, _ = self.h("apps")
        self.assertNotEqual(c0, c1)
        self.assertNotEqual(a0, a1)

    def test_lock_file_moves_both(self):
        c0, _ = self.h("core"); a0, _ = self.h("apps")
        write(self.root, "Cargo.lock", "# lock 2\n"); commit(self.root)
        self.assertNotEqual(c0, self.h("core")[0])
        self.assertNotEqual(a0, self.h("apps")[0])

    def test_docs_and_contract_files_move_core_only_and_site_nothing(self):
        c0, _ = self.h("core"); a0, _ = self.h("apps")
        write(self.root, "FEATURES.md", "features 2\n"); commit(self.root)
        c1, _ = self.h("core")
        self.assertNotEqual(c0, c1)
        self.assertEqual(a0, self.h("apps")[0])
        write(self.root, "site/index.html", "site 2\n"); commit(self.root)
        self.assertEqual(c1, self.h("core")[0])

    def test_uncommitted_and_untracked_inputs_refuse_a_hash(self):
        write(self.root, "crates/iyakki-core/src/lib.rs", "// edited, not committed\n")
        h, why = self.h("core")
        self.assertIsNone(h)
        self.assertIn("uncommitted", why)
        sh(self.root, "checkout", "--", ".")
        write(self.root, "crates/iyakki-tile-one/src/new.rs", "// untracked\n")
        self.assertIsNone(self.h("apps")[0])

    def test_stamp_cycle(self):
        ok, why = tl.verify(self.root, "core", self.crates, self.rules)
        self.assertFalse(ok)
        self.assertEqual(why, "no stamp")
        h, _ = self.h("core")
        tl.write_stamp(self.root, "core", self.crates, self.rules, h)
        ok, st = tl.verify(self.root, "core", self.crates, self.rules)
        self.assertTrue(ok)
        self.assertEqual(st["commit"], tl.git(self.root, "rev-parse", "HEAD").strip())
        self.assertFalse(tl.verify(self.root, "apps", self.crates, self.rules)[0])  # the other layer has none
        write(self.root, "crates/iyakki-core/src/lib.rs", "// new\n"); commit(self.root)
        ok, why = tl.verify(self.root, "core", self.crates, self.rules)
        self.assertFalse(ok)
        self.assertIn("inputs changed", why)

    def test_a_stamp_survives_a_commit_that_changes_nothing_it_reads(self):
        h, _ = self.h("apps")
        tl.write_stamp(self.root, "apps", self.crates, self.rules, h)
        write(self.root, "docs/a.md", "doc 2\n"); commit(self.root)
        self.assertTrue(tl.verify(self.root, "apps", self.crates, self.rules)[0])

    def test_stamp_refused_when_inputs_moved_or_are_dirty(self):
        h, _ = self.h("core")
        write(self.root, "crates/iyakki-lib/src/lib.rs", "// dirty\n")
        with self.assertRaises(tl.LayerError):
            tl.write_stamp(self.root, "core", self.crates, self.rules, h)
        commit(self.root)
        with self.assertRaises(tl.LayerError):  # committed, but not the tree the tests ran on
            tl.write_stamp(self.root, "core", self.crates, self.rules, h)
        self.assertIsNone(tl.read_stamp(self.root, "core"))

    def test_status_cli(self):
        env = dict(os.environ, IYAKKI_ROOT=self.root)
        run = lambda *a: subprocess.run([sys.executable, "-I", os.path.join(HERE, "testlayers.py")] + list(a), env=env, capture_output=True, text=True)
        self.assertEqual(run("status").returncode, 1)
        for g in ("core", "apps"):
            h = run("hash", g).stdout.strip()
            self.assertEqual(run("stamp", g, "--expect", h).returncode, 0)
        p = run("status")
        self.assertEqual(p.returncode, 0, p.stdout)
        self.assertIn("GREEN", p.stdout)
        self.assertEqual(run("crates", "apps", "--p").stdout.strip(), "-p iyakki-shell -p iyakki-tile-one -p iyakki-tile-two")


class Runner(unittest.TestCase):
    """dist/release/test-layer.sh in a fake repo with a fake cargo."""

    def setUp(self):
        self.root = fake_repo()
        for rel in ("dist/release/env.sh", "dist/release/test-layer.sh", "dist/release/test-with-rerun.sh", "dist/scripts/testlayers.py"):
            write(self.root, rel, slurp(os.path.join(REAL, rel)))
            os.chmod(os.path.join(self.root, rel), 0o755)
        write(self.root, "dist/release/known-flaky.txt", "")
        sh(self.root, "add", "-A")
        commit(self.root, "scripts")
        self.bin = tempfile.mkdtemp(prefix="iyakki-fakebin-")
        with open(os.path.join(self.bin, "cargo"), "w") as f:
            f.write(CARGO)
        os.chmod(os.path.join(self.bin, "cargo"), 0o755)
        self.log = os.path.join(self.bin, "log")
        self.failflag = os.path.join(self.bin, "fail")
        self.env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ["PATH"], FAKE_CARGO_LOG=self.log, FAKE_CARGO_FAIL=self.failflag, IYAKKI_TMP_SET="")
        self.env.pop("IYAKKI_STAMP_DIR", None)

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)
        shutil.rmtree(self.bin, ignore_errors=True)

    def layer(self, name, *a):
        p = subprocess.run([os.path.join(self.root, "dist/release/test-layer.sh"), name] + list(a), env=self.env, capture_output=True, text=True, cwd=self.root)
        return p.returncode, p.stdout + p.stderr

    def calls(self):
        return slurp(self.log).splitlines() if os.path.exists(self.log) else []

    def stamp(self, g):
        return os.path.exists(os.path.join(self.root, "target/alpha-check/stamps", g))

    def test_pass_stamps_then_skips_then_no_stamps_forces(self):
        rc, out = self.layer("apps")
        self.assertEqual(rc, 0, out)
        self.assertTrue(self.stamp("apps"))
        self.assertFalse(self.stamp("core"))
        first = self.calls()
        self.assertEqual(first, ["test -p iyakki-shell -p iyakki-tile-one -p iyakki-tile-two --locked --no-fail-fast"])
        rc, out = self.layer("apps")
        self.assertEqual(rc, 10, out)
        self.assertRegex(out, r"SKIP stamp [0-9a-f]{12}")
        self.assertEqual(self.calls(), first)  # cargo was not run again
        rc, out = self.layer("apps", "--no-stamps")
        self.assertEqual(rc, 0, out)
        self.assertEqual(len(self.calls()), 2)

    def test_core_crate_list_comes_from_the_layer_file(self):
        self.layer("core")
        self.assertEqual(self.calls()[0], "test -p iyakki-core -p iyakki-lib --locked --no-fail-fast")

    def test_failed_run_writes_no_stamp(self):
        open(self.failflag, "w").close()
        rc, out = self.layer("core")
        self.assertEqual(rc, 1, out)
        self.assertFalse(self.stamp("core"))

    def test_an_old_stamp_does_not_survive_a_changed_layer(self):
        self.assertEqual(self.layer("core")[0], 0)
        write(self.root, "crates/iyakki-lib/src/lib.rs", "// x\n"); commit(self.root)
        rc, out = self.layer("core")
        self.assertEqual(rc, 0, out)  # ran again
        self.assertEqual(len(self.calls()), 2)

    def test_partial_run_and_dirty_tree_write_no_stamp(self):
        rc, out = self.layer("apps", "--", "some_filter")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.stamp("apps"))
        write(self.root, "crates/iyakki-tile-two/src/lib.rs", "// uncommitted\n")
        rc, out = self.layer("apps")
        self.assertEqual(rc, 0, out)
        self.assertFalse(self.stamp("apps"))
        self.assertIn("uncommitted", out)

    def test_a_crate_without_a_layer_stops_the_runner(self):
        write(self.root, "dist/scripts/crate-layers.txt", "iyakki-lib base\n")
        commit(self.root)
        rc, out = self.layer("core")
        self.assertEqual(rc, 1, out)
        self.assertEqual(self.calls(), [])


class RealRepo(unittest.TestCase):
    def test_every_real_workspace_crate_has_a_layer(self):
        env = dict(os.environ, IYAKKI_ROOT=REAL)
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "testlayers.py"), "check"], env=env, capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)


if __name__ == "__main__":
    unittest.main()
