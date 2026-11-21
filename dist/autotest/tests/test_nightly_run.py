import datetime
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import nightly  # noqa: E402
import results as R  # noqa: E402
import run  # noqa: E402

NOW = datetime.datetime(2026, 11, 22, 7, 0, 0)


def rec(status, when, run_id, commit="aaa", level="qemu", row="F-T-1"):
    r = R.new_result(row, "F-T-1", "qemu" if level == "qemu" else "real", status, commit=commit, run_id=run_id, ran_at=when, device="" if level == "qemu" else "lenovo")
    r["_dir"] = "/ev/" + run_id
    return r


class Triage(unittest.TestCase):
    def test_two_fails_in_a_row_draft_a_known_issue(self):
        t = nightly.triage([rec("PASS", "2026-11-20T01:00:00", "r1", "good1"), rec("FAIL", "2026-11-21T01:00:00", "r2", "c2"), rec("FAIL", "2026-11-22T01:00:00", "r3", "c3")], NOW)
        self.assertEqual(len(t["drafts"]), 1)
        self.assertEqual(t["drafts"][0]["last_good_commit"], "good1")

    def test_one_fail_or_a_fail_between_passes_is_no_draft(self):
        self.assertEqual(nightly.triage([rec("PASS", "2026-11-20T01:00:00", "r1"), rec("FAIL", "2026-11-21T01:00:00", "r2")], NOW)["drafts"], [])
        self.assertEqual(nightly.triage([rec("FAIL", "2026-11-20T01:00:00", "r1"), rec("PASS", "2026-11-21T01:00:00", "r2"), rec("FAIL", "2026-11-22T01:00:00", "r3")], NOW)["drafts"], [])

    def test_a_skip_or_blocked_between_two_fails_does_not_hide_them_and_does_not_count(self):
        t = nightly.triage([rec("FAIL", "2026-11-20T01:00:00", "r1"), rec("BLOCKED", "2026-11-21T01:00:00", "r2"), rec("FAIL", "2026-11-22T01:00:00", "r3")], NOW)
        self.assertEqual(len(t["drafts"]), 1)

    def test_pass_and_fail_on_one_commit_is_a_flake(self):
        t = nightly.triage([rec("PASS", "2026-11-21T01:00:00", "r1", "same"), rec("FAIL", "2026-11-22T01:00:00", "r2", "same")], NOW)
        self.assertEqual(len(t["flakes"]), 1)
        self.assertEqual(nightly.triage([rec("PASS", "2026-11-21T01:00:00", "r1", "a"), rec("FAIL", "2026-11-22T01:00:00", "r2", "b")], NOW)["flakes"], [])

    def test_levels_are_kept_apart(self):
        t = nightly.triage([rec("FAIL", "2026-11-21T01:00:00", "r1"), rec("FAIL", "2026-11-22T01:00:00", "r2", level="machine")], NOW)
        self.assertEqual(t["drafts"], [])

    def test_old_evidence_is_flagged_expired(self):
        t = nightly.triage([rec("PASS", "2026-10-01T01:00:00", "r1")], NOW)
        self.assertEqual(t["expired"][0]["last_pass"], "2026-10-01")

    def test_files_are_written_and_no_ki_number_is_invented(self):
        with tempfile.TemporaryDirectory() as d:
            t = nightly.triage([rec("FAIL", "2026-11-21T01:00:00", "r1"), rec("FAIL", "2026-11-22T01:00:00", "r2")], NOW)
            paths = nightly.write_triage(t, os.path.join(d, "drafts"), os.path.join(d, "out"), "20261122")
            text = open(paths[0]).read()
            self.assertIn("KI-DRAFT-F-T-1-qemu", paths[0])
            self.assertNotRegex(text, r"KI-\d+")
            self.assertTrue(any(p.endswith("morning.txt") for p in paths))


class Schedule(unittest.TestCase):
    def test_plan_has_the_four_steps_in_order(self):
        lines = nightly.plan_text().splitlines()
        self.assertEqual([l.split()[0] for l in lines], ["01:00", "02:30", "04:00", "06:30"])

    def test_wait_for_uses_the_injected_clock_not_a_sleep(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "x.DONE")
            now = [0]
            def sleep(s):
                now[0] += s
                if now[0] >= 90:
                    open(p, "w").close()
            self.assertTrue(nightly.wait_for(p, 1000, clock=lambda: now[0], sleep=sleep))
            self.assertFalse(nightly.wait_for(os.path.join(d, "never"), 100, clock=lambda: now[0] + 100, sleep=sleep))

    def test_install_writes_text_and_enables_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            nightly.main(["install", "--dir", d])
            names = sorted(os.listdir(d))
            self.assertEqual(len(names), 8)
            self.assertIn("OnCalendar=*-*-* 02:30:00", open(os.path.join(d, "iyakki-autotest-qemu.timer")).read())

    def test_one_lock(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "l")
            a = nightly.take_lock(p)
            self.assertIsNotNone(a)
            self.assertIsNone(nightly.take_lock(p))
            a.close()
            b = nightly.take_lock(p)
            self.assertIsNotNone(b)
            b.close()

    def test_a_step_waits_for_the_previous_one(self):
        with tempfile.TemporaryDirectory() as d:
            calls = []
            os.makedirs(os.path.join(d, "nightly"))
            # no gate DONE and the wait deadline is past: the step does not run
            real = nightly.wait_for
            nightly.wait_for = lambda *a, **k: False
            self.addCleanup(lambda: setattr(nightly, "wait_for", real))
            self.assertEqual(nightly.run_step("qemu", "img", d, d, log=lambda s: None, runner=lambda c: calls.append(c) or 0), 2)
            self.assertEqual(calls, [])


class RunnerPlanning(unittest.TestCase):
    def test_status_mapping(self):
        m = run.map_status
        self.assertEqual(m({"status": "pass"})[0], "PASS")
        self.assertEqual(m({"status": "pass"}, new_golden=True)[0], "NEW-GOLDEN")
        self.assertEqual(m({"status": "fail", "reason": "x"})[0], "FAIL")
        self.assertEqual(m({"status": "error", "reason": "x"})[0], "FAIL")
        self.assertEqual(m({"status": "skipped", "reason": "not run: 'Sign in' is in front"})[0], "BLOCKED")
        self.assertEqual(m({"status": "skipped", "reason": "scenario supports qemu only"})[0], "NOT-APPLICABLE")
        self.assertEqual(m({"status": "skipped", "reason": "no button"})[0], "SKIP")

    def test_quarantine_turns_a_pass_into_blocked(self):
        r = [rec("PASS", "2026-11-22T01:00:00", "r1")]
        run.apply_quarantine(r, {"F-T-1"})
        self.assertEqual(r[0]["status"], "BLOCKED")
        self.assertEqual(r[0]["rung_claim"], 0)

    def test_every_shipped_scenario_follows_the_format(self):
        entries = run.load_scenarios()
        idx = run.features_index()
        self.assertEqual(run.problems(entries, set(idx) or None), [])

    def test_the_quarantine_file_names_only_real_scenarios(self):
        names = {os.path.splitext(os.path.basename(e["file"]))[0] for e in run.load_scenarios()}
        self.assertLessEqual(run.quarantined(), names)

    def test_selection_by_profile_and_rows(self):
        entries = run.load_scenarios()
        for e in entries:
            self.assertTrue(e["rows"])
        some = entries[0]["rows"][0] if entries else "F-NONE-1"
        ran, _na = run.select(entries, "qemu", [some])
        self.assertTrue(all(some in e["rows"] for e in ran))

    def test_image_commit_comes_from_the_argument_or_the_gate_result_never_guessed(self):
        self.assertEqual(run.image_commit("/x", "abc1234"), ("abc1234", "argument"))
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "pack", "images"))
            open(os.path.join(d, "RESULT.txt"), "w").write("RESULT: PASS\ncommit: b741a1ba5496b4d9983415a85340f327ad26fe18\n")
            self.assertEqual(run.image_commit(os.path.join(d, "pack"))[1], "RESULT.txt")
        self.assertEqual(run.image_commit("/nonexistent/x")[1], "unknown")

    def test_disk_guard_and_lock(self):
        self.assertTrue(run.disk_ok("/tmp/does/not/exist", limit=1.01))
        self.assertFalse(run.disk_ok("/tmp", limit=0.0))
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "l")
            a = run.take_lock(p)
            self.assertIsNone(run.take_lock(p))
            a.close()
            self.assertFalse(run.gate_running(os.path.join(d, "absent")))
            g = run.take_lock(os.path.join(d, "g"))
            self.assertTrue(run.gate_running(os.path.join(d, "g")))
            g.close()

    def test_a_machine_in_use_is_blocked_and_the_wrong_hardware_is_not_applicable(self):
        class St:
            app = "Sign in"
        class D:
            def try_state(self):
                return St()
        ok, why = run.device_gate(D())
        self.assertFalse(ok)
        self.assertTrue(why.startswith("not run:"))
        e = {"targets": ["real", "hw:macmini2018"]}
        self.assertFalse(run.hw_ok(e, "device   Macmini7,1\n")[0])
        self.assertTrue(run.hw_ok(e, "device   Macmini8,1\n")[0])


if __name__ == "__main__":
    unittest.main()
