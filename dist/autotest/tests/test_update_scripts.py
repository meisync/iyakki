import importlib.util
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
sys.path.insert(0, os.path.join(HERE, ".."))
import results as R  # noqa: E402
import run  # noqa: E402


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), os.path.join(ROOT, "dist", "qemu", name + ".py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


MENU = 'set timeout="3"\nmenuentry "IYAKKI" {\n\tlinux /boot/bzImage quiet loglevel=3\n\tinitrd /boot/initrd\n}\n'


class Faults(unittest.TestCase):
    def test_the_previous_entry_is_the_normal_entry_with_the_marker_and_the_engines_own_entry_is_replaced(self):
        m = load("update-faults-test")
        out = m.fix_menu(MENU)
        self.assertEqual(out.count('menuentry "IYAKKI (previous version)"'), 1)
        self.assertIn("loglevel=7 iyakki.entry=previous", out)
        self.assertIn('set timeout="1"', out)
        engine = MENU.replace("{", "{", 1) + 'menuentry "IYAKKI (previous version)" {\n\tlinux (hd0,2)/boot-prev/x/bzImage quiet\n}\n'
        again = m.fix_menu(engine)
        self.assertEqual(again.count("(previous version)"), 1)
        self.assertNotIn("boot-prev", again)

    def test_a_grub_core_goes_to_sector_one_of_the_stick_and_an_oversized_one_is_refused(self):
        m = load("update-faults-test")
        with tempfile.TemporaryDirectory() as d:
            img, core = os.path.join(d, "stick.img"), os.path.join(d, "grub.img")
            open(img, "wb").write(b"M" * 512 + b"\0" * 4096 + b"P" * 512)
            open(core, "wb").write(b"C" * 700)
            m.put_grub_core(img, core)
            data = open(img, "rb").read()
            self.assertEqual(data[:512], b"M" * 512, "the boot sector stays")
            self.assertEqual(data[512:1212], b"C" * 700)
            self.assertEqual(data[-512:], b"P" * 512, "what follows the core is not cut")
            self.assertEqual(len(data), 512 + 4096 + 512, "the image keeps its size")
            open(core, "wb").write(b"C" * (2047 * 512 + 1))
            with self.assertRaises(SystemExit):
                m.put_grub_core(img, core)

    def test_every_case_expects_three_starts_and_an_armed_counter_always_falls_back_on_the_second(self):
        m = load("update-faults-test")
        for name, (counter, entries) in m.CASES.items():
            self.assertEqual(len(entries), 3, name)
            self.assertEqual(entries[1] == "previous", counter == "Armed", name)
            self.assertEqual(entries[0], "normal", name)
        self.assertEqual(set(m.CASES), {"control", "done", "cut-initrd", "cut-kernel", "cut-menu"})


LOADER_CFG = ("tries 9\n\nentry iyakki\n  title IYAKKI\n  cmdline console=ttyS0 quiet loglevel=3 x\n  default\n\nentry prev-plain\n  cmdline console=ttyS0 quiet loglevel=3 x\n"
              "  requires /EFI/iyakki-boot/prev/set-plain\n\nentry safe\n  cmdline console=ttyS0 quiet loglevel=3 nomodeset\n")


class LoaderLayout(unittest.TestCase):
    def test_the_copys_boot_cfg_prints_the_kernel_command_line_and_marks_only_the_previous_rows(self):
        m = load("update-faults-test")
        out = m.fix_loader_cfg(LOADER_CFG)
        entries = out.split("\nentry ")
        self.assertIn("loglevel=7 x\n", entries[1] + "\n")
        self.assertNotIn("iyakki.entry=previous", entries[1])
        self.assertTrue(entries[2].split("\n")[1].rstrip().endswith("iyakki.entry=previous"), entries[2])
        self.assertIn("quiet loglevel=3 nomodeset", entries[3], "the other rows are not touched")
        self.assertIn("tries 9", out)
        self.assertIn("requires /EFI/iyakki-boot/prev/set-plain", out)

    def test_the_fault_state_line_of_the_engine_is_read_with_its_prev_listing_and_marker(self):
        m = load("update-faults-test")
        st = m.parse_state('FAULT-STATE case=done apply=ok counter=Armed files=new prev=[bzImage:12829696,initrd:71897804,set-plain:33] marker="version 0.1.0-alpha.2;stamp 1;"')
        self.assertEqual((st["files"], st["prev_files"]["bzImage"], st["prev_files"]["initrd"]), ("new", 12829696, 71897804))
        self.assertEqual(st["marker_text"], "version 0.1.0-alpha.2;stamp 1;")
        empty = m.parse_state('FAULT-STATE case=control apply=ok counter=Idle files=old prev=[] marker=""')
        self.assertEqual((empty["prev_files"], empty["marker_text"]), ({}, ""))

    def test_the_loader_cases_name_the_set_that_stands_and_only_a_whole_copy_has_a_marker(self):
        m = load("update-faults-test")
        self.assertEqual(set(m.LOADER_CASES), {"control", "done", "cut-initrd", "cut-kernel", "cut-menu", "cut-prev-kernel", "cut-prev-initrd", "cut-prev-marker"})
        for name, (files, marker, kind) in m.LOADER_CASES.items():
            self.assertEqual(marker, kind == "whole", name)
            self.assertIn(files, ("old", "new", "mixed"), name)
            if name.startswith("cut-prev-") or name == "control":
                self.assertEqual(files, "old", "nothing of the new set was written before the loader's copy was complete: " + name)
        self.assertEqual(m.LOADER_CASES["cut-kernel"][0], "mixed", "the new initramfs stands beside the old kernel")

    def test_the_loader_scenario_runs_the_same_script_with_the_loader_flag(self):
        by_row = {r: e for e in run.load_scenarios() for r in e["rows"]}
        self.assertEqual(by_row["F-UPD-31"]["meta"]["host-script"], "dist/qemu/update-faults-test.py --loader")


class HostScripts(unittest.TestCase):
    def test_a_host_script_is_run_with_the_out_folder_and_its_own_record_is_read_back_and_a_silent_script_is_blocked(self):
        with tempfile.TemporaryDirectory() as d:
            good = os.path.join(d, "good.py")
            open(good, "w").write("import sys, os, json\nsys.path.insert(0, %r)\nimport results as R\nout = sys.argv[sys.argv.index('--out') + 1]\n"
                                   "R.write_result(out, R.new_result('F-T-1', 's', 'qemu', 'PASS', evidence=['e.txt']))\n" % os.path.join(ROOT, "dist", "autotest"))
            silent = os.path.join(d, "silent.py")
            open(silent, "w").write("import sys\nsys.exit(2)\n")
            entries = [{"file": "/x/F-T-1.scn", "rows": ["F-T-1"], "meta": {"host-script": os.path.relpath(good, ROOT)}},
                       {"file": "/x/F-T-2.scn", "rows": ["F-T-2"], "meta": {"host-script": os.path.relpath(silent, ROOT)}}]
            out = os.path.join(d, "out")
            saved, run.ROOT = run.ROOT, ROOT      # another test file may have pointed the runner's root at a scratch folder
            try:
                recs = run.run_host_scripts(entries, out, log=lambda *_: None)
            finally:
                run.ROOT = saved
            self.assertEqual([r["status"] for r in recs], ["PASS", "BLOCKED"])
            self.assertTrue(os.path.isfile(os.path.join(out, "F-T-1", "qemu", "result.json")))
            self.assertEqual(R.validate(recs[1]), [])

    def test_the_real_scenario_files_name_their_host_script_in_the_header(self):
        # Q1 2026-11-21: `host-script` was not a header key, so the runner took it for a step ("unknown step") and the two U1 rows failed instead of running their scripts
        by_row = {r: e for e in run.load_scenarios() for r in e["rows"]}
        self.assertEqual(by_row["F-UPD-28"]["meta"]["host-script"], "dist/qemu/update-faults-test.py")
        self.assertEqual(by_row["F-UPD-29"]["meta"]["host-script"], "dist/qemu/upgrade-test.py")

    def test_an_unlocked_lock_file_is_not_a_running_gate(self):
        import fcntl
        with tempfile.TemporaryDirectory() as d:
            lock = os.path.join(d, "gate.lock")
            self.assertFalse(R.gate_lock_held(lock), "no file")
            open(lock, "w").close()
            self.assertFalse(R.gate_lock_held(lock), "a file nobody holds")
            with open(lock, "a") as fh:
                fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.assertTrue(R.gate_lock_held(lock), "a file the gate holds")
            self.assertFalse(R.gate_lock_held(lock), "released")

    def test_a_session_check_that_changes_the_guest_runs_once_for_all_its_scenarios(self):
        import types
        calls = []
        d0 = types.ModuleType("session_checks_defs"); d1 = types.ModuleType("session_checks_ui_defs")
        d1.check_once = lambda s: calls.append(1) or "the permission was taken away"
        saved = {k: sys.modules.get(k) for k in ("session_checks_defs", "session_checks_ui_defs")}
        sys.modules["session_checks_defs"], sys.modules["session_checks_ui_defs"] = d0, d1
        try:
            with tempfile.TemporaryDirectory() as d:
                es = [{"file": "/x/F-T-1.scn", "rows": ["F-T-1"], "meta": {"session-check": "once", "row": "F-T-1"}},
                      {"file": "/x/F-T-2.scn", "rows": ["F-T-2"], "meta": {"session-check": "once", "row": "F-T-2"}}]
                recs = run.run_session_checks(es, object(), d, {"run_id": "r", "commit": "abc", "image": "", "commit_source": "argument"}, {}, log=lambda *_: None)
                self.assertEqual(len(calls), 1)
                self.assertEqual([r["status"] for r in recs], ["PASS", "PASS"])
                self.assertIn("one run of this check serves every scenario", open(os.path.join(d, "F-T-2", "qemu", "F-T-2", "session-check-once.txt")).read())
        finally:
            for k, v in saved.items():
                if v is None:
                    sys.modules.pop(k, None)
                else:
                    sys.modules[k] = v


if __name__ == "__main__":
    unittest.main()
