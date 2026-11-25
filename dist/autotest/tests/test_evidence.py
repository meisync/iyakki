import datetime
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import evidence  # noqa: E402
import results as R  # noqa: E402

NOW = datetime.datetime(2026, 11, 22, 12, 0, 0)
FEATURE = "The bell counts only approvals"
HEAD = "| ID | Feature | Phase | Status | Pri | Impl/Test | Manual | Tests | Notes |\n|---|---|---|---|---|---|---|---|---|\n"


def features(test="50", manual="none", impl="100", status="partial", tests="`crate::a::b`"):
    return HEAD + "| F-T-1 | %s | P5 | %s | P1 | %s/%s | %s | %s | n |\n" % (FEATURE, status, impl, test, manual, tests)


class Fix:
    """A throw-away repository root (with the scenario file) and a results folder."""

    def __init__(self, tc):
        self.tmp = tempfile.TemporaryDirectory()
        tc.addCleanup(self.tmp.cleanup)
        self.repo = os.path.join(self.tmp.name, "repo")
        self.out = os.path.join(self.tmp.name, "out")
        os.makedirs(os.path.join(self.repo, "dist/autotest/scenarios"))
        open(os.path.join(self.repo, "dist/autotest/scenarios/F-T-1.scn"), "w").write("row: F-T-1\n")
        self.n = 0

    def add(self, run, status="PASS", target="qemu", commit="abc12345", when="2026-11-21T10:00:00", **kw):
        d = os.path.join(self.out, run, "F-T-1", R.level_of(target, kw.get("device", "")))
        os.makedirs(d, exist_ok=True)
        open(os.path.join(d, "01-home.png"), "wb").write(b"png-bytes")
        r = R.new_result("F-T-1", "F-T-1", target, status, commit=commit, image="img", image_commit_source="RESULT.txt", run_id=run, ran_at=when,
                         evidence=["01-home.png"], covered=["1 the bell counts only approvals"], clauses_attested=True,
                         scenario_file="dist/autotest/scenarios/F-T-1.scn")
        r["feature_sha"] = R.feature_sha(FEATURE)
        r.update(kw)
        r["level"] = R.level_of(target, r["device"])
        json.dump(r, open(os.path.join(d, "result.json"), "w"))
        return d

    def run(self, text, **kw):
        return evidence.apply_to_text(text, self.out, NOW, self.repo, **kw)


class SeventyRule(unittest.TestCase):
    def test_two_consecutive_passes_on_the_image_commit_raise_50_to_70(self):
        f = Fix(self)
        f.add("run1")
        f.add("run2", when="2026-11-22T02:30:00")
        new, rep = f.run(features(), image_commit="abc12345")
        self.assertEqual(rep[0][1:3], (50, 70), rep)
        self.assertIn("| 100/70 |", new)
        self.assertIn("`script:dist/autotest/scenarios/F-T-1.scn`", new)
        self.assertIn("`crate::a::b`", new)

    def refused(self, why, text=None, **kw):
        new, rep = self.f.run(text or features(), **kw)
        self.assertIsNone(rep[0][2], rep)
        self.assertIn(why, rep[0][3])
        self.assertEqual(new, text or features(), "nothing may change")

    def setUp(self):
        self.f = Fix(self)

    def two(self, **kw2):
        self.f.add("run1")
        self.f.add("run2", when="2026-11-22T02:30:00", **kw2)

    def test_a_program_over_an_older_base_counts_only_when_the_scenario_says_it_provides_the_clauses(self):
        f = Fix(self)
        f.add("run1", base_commit="0f987f0c")
        f.add("run2", when="2026-11-22T02:30:00", base_commit="0f987f0c")
        new, rep = f.run(features(), image_commit="abc12345")
        self.assertIsNone(rep[0][2], rep)
        self.assertIn("provided-by: program", rep[0][3])
        self.assertIn("| 100/50 |", new)
        open(os.path.join(f.repo, "dist/autotest/scenarios/F-T-1.scn"), "w").write("row: F-T-1\nprovided-by: program  # the iyakki binary gives every clause\n")
        new, rep = f.run(features(), image_commit="abc12345")
        self.assertEqual(rep[0][1:3], (50, 70), rep)
        self.assertIn("program abc12345 over base 0f987f0c", rep[0][3])

    def test_a_result_with_the_same_base_and_program_needs_no_header(self):
        f = Fix(self)
        f.add("run1", base_commit="abc12345")
        f.add("run2", when="2026-11-22T02:30:00", base_commit="abc12345")
        _new, rep = f.run(features(), image_commit="abc12345")
        self.assertEqual(rep[0][1:3], (50, 70), rep)

    def test_one_pass_is_not_enough(self):
        self.f.add("run1")
        self.refused("one PASS so far")

    def test_both_results_from_one_run_do_not_count_twice(self):
        self.f.add("run1", when="2026-11-21T10:00:00")
        d = os.path.join(self.f.out, "run1", "F-T-1", "browser")
        self.f.add("run1", target="browser", when="2026-11-22T02:30:00")
        self.assertTrue(os.path.isdir(d))
        self.refused("one run")

    def test_nothing_is_raised_on_skip_blocked_not_applicable_needs_human_or_new_golden(self):
        for st in ("SKIP", "BLOCKED", "NOT-APPLICABLE", "NEEDS-HUMAN", "NEW-GOLDEN", "FAIL"):
            f = Fix(self)
            f.add("run1")
            f.add("run2", status=st, when="2026-11-22T02:30:00", evidence=["01-home.png"])
            new, rep = f.run(features())
            self.assertIsNone(rep[0][2], st)
            self.assertIn("latest qemu result is " + st, rep[0][3], st)
            self.assertEqual(new, features())

    def test_a_non_empty_not_covered_refuses(self):
        self.two(not_covered=["clause 2: the badge"])
        self.refused("not covered")

    def test_an_empty_covered_list_refuses(self):
        self.two(covered=[])
        self.refused("no clause")

    def test_a_changed_feature_text_makes_the_clause_list_stale(self):
        self.two(feature_sha="0" * 16)
        self.refused("another Feature text")

    def test_unattested_clauses_refuse(self):
        self.two(clauses_attested=False)
        self.refused("another Feature text")

    def test_old_evidence_expires_and_is_not_lowered(self):
        self.f.add("run1", when="2026-10-01T10:00:00")
        self.f.add("run2", when="2026-10-02T10:00:00")
        self.refused("expired")

    def test_another_commit_than_the_image_refuses(self):
        self.two()
        self.refused("image commit", image_commit="ffff0000")

    def test_two_different_commits_refuse(self):
        self.f.add("run1", commit="aaaa0001")
        self.f.add("run2", commit="bbbb0002", when="2026-11-22T02:30:00")
        self.refused("image commit")

    def test_an_unknown_image_commit_refuses(self):
        self.two(image_commit_source="unknown")
        self.refused("not known")

    def test_a_flaky_flag_refuses(self):
        self.two(flaky=True)
        self.refused("flaky")

    def test_a_pass_and_a_fail_on_the_same_commit_is_flaky(self):
        self.f.add("run0", status="FAIL", when="2026-11-20T10:00:00")
        self.f.add("run1")
        self.f.add("run2", when="2026-11-22T02:30:00")
        self.refused("flaky")

    def test_missing_or_empty_evidence_refuses(self):
        self.two()
        os.truncate(os.path.join(self.f.out, "run2", "F-T-1", "qemu", "01-home.png"), 0)
        self.refused("missing or empty")

    def test_evidence_must_stay_inside_the_result_folder(self):
        self.two(evidence=["../../../x.png"])
        self.refused("leaves the result folder")

    def test_a_missing_scenario_file_refuses(self):
        self.two(scenario_file="dist/autotest/scenarios/nope.scn")
        self.refused("not in the repository")

    def test_impl_below_100_refuses(self):
        self.two()
        self.refused("Impl is 90", text=features(impl="90"))

    def test_a_row_already_at_70_or_more_is_not_touched(self):
        self.two()
        new, rep = self.f.run(features(test="70"))
        self.assertEqual(new, features(test="70"))
        self.assertIsNone(rep[0][2])

    def test_a_pass_on_the_real_target_alone_does_not_give_70(self):
        self.f.add("run1", target="real")
        self.f.add("run2", target="real", when="2026-11-22T02:30:00")
        self.refused("no qemu or browser")

    def test_never_lowers(self):
        self.f.add("run1", status="FAIL")
        new, rep = self.f.run(features(test="70"))
        self.assertEqual(new, features(test="70"))


class HundredRule(unittest.TestCase):
    def setUp(self):
        self.f = Fix(self)

    def test_qemu_evidence_never_sets_100_by_default(self):
        self.f.add("run1", screen_evidence=True)
        self.f.add("run2", when="2026-11-22T02:30:00", screen_evidence=True)
        new, rep = self.f.run(features(test="70"))
        self.assertIsNone(rep[0][2])
        self.assertIn("needs a real machine", rep[0][3])
        self.assertEqual(new, features(test="70"))

    def test_the_switch_sets_100_for_a_software_row_with_screen_evidence_and_makes_it_done(self):
        self.f.add("run1", screen_evidence=True)
        self.f.add("run2", when="2026-11-22T02:30:00", screen_evidence=True)
        new, rep = self.f.run(features(test="70"), qemu_counts_as_real=True)
        self.assertEqual(rep[0][1:3], (70, 100))
        self.assertIn("| done |", new)

    def test_the_switch_does_not_help_a_manual_needed_row_or_one_without_screen_evidence(self):
        self.f.add("run1", screen_evidence=True)
        self.f.add("run2", when="2026-11-22T02:30:00", screen_evidence=True)
        _n, rep = self.f.run(features(test="70", manual="needed"), qemu_counts_as_real=True)
        self.assertIsNone(rep[0][2])
        g = Fix(self)
        g.add("run1")
        g.add("run2", when="2026-11-22T02:30:00")
        _n, rep = g.run(features(test="70"), qemu_counts_as_real=True)
        self.assertIsNone(rep[0][2])

    def real(self, **kw):
        base = dict(device="lenovo-t7", device_ok=True, covered=["1 the bell counts only approvals"])
        base.update(kw)
        return self.f.add("run3", target="real", when="2026-11-22T04:00:00", **base)

    def human(self, clause, machine="lenovo-t7", result="pass", by="owner"):
        os.makedirs(os.path.join(self.f.out, "human"), exist_ok=True)
        with open(evidence.human_path(os.path.join(self.f.out, "human"), "F-T-1", clause, machine), "w") as fh:
            json.dump({"schema": 1, "row": "F-T-1", "clause": clause, "machine": machine, "by": by, "verdict": result, "note": "", "date": "2026-11-22",
                       "image_commit": "abc12345"}, fh)

    def test_a_real_machine_pass_sets_100_and_the_manual_column(self):
        self.real()
        new, rep = self.f.run(features(test="70", manual="needed"))
        self.assertEqual(rep[0][2], 100, rep)
        self.assertIn("| passed 2026-11-22 lenovo-t7 |", new)
        self.assertIn("| done |", new)

    def test_a_real_pass_on_the_wrong_machine_gives_nothing(self):
        self.real(device_ok=False)
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertIsNone(rep[0][2])

    def test_a_human_clause_needs_the_owners_pass_for_that_machine(self):
        self.real(needs_human=["ears"])
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertIsNone(rep[0][2])
        self.assertIn("no human result", rep[0][3])
        self.human("ears", machine="macmini-disk")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertIsNone(rep[0][2])
        self.human("ears", result="fail")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertIsNone(rep[0][2])
        self.human("ears")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertEqual(rep[0][2], 100, rep)

    def human_at(self, clause, folder=None, **kw):
        """A human file written into human/ or, like an import, into human/<folder>/."""
        base = os.path.join(self.f.out, "human", folder) if folder else os.path.join(self.f.out, "human")
        os.makedirs(base, exist_ok=True)
        rec = {"schema": 1, "row": "F-T-1", "clause": clause, "machine": "lenovo-t7", "by": "owner", "verdict": "pass", "date": "2026-11-22", "image_commit": "abc12345"}
        rec.update(kw)
        with open(os.path.join(base, "F-T-1.%s.%s.json" % (clause, rec["machine"])), "w") as fh:
            json.dump(rec, fh)

    def test_every_role_of_the_allow_list_is_a_human_pass(self):
        for role in ("owner", "admin", "person"):
            g = Fix(self)
            g.add("run1")
            g.add("run2", when="2026-11-22T02:30:00")
            self.f = g
            self.real(needs_human=["ears"])
            self.human("ears", by=role)
            _n, rep = g.run(features(test="70", manual="needed"))
            self.assertEqual(rep[0][2], 100, (role, rep))

    def test_a_name_or_an_unknown_role_is_not_a_human_pass(self):
        self.real(needs_human=["ears"])
        for who in ("ana", "root", ""):
            self.human("ears", by=who)
            _n, rep = self.f.run(features(test="70", manual="needed"))
            self.assertIsNone(rep[0][2], who)
            self.assertIn("breaks the schema", rep[0][3])

    def test_a_record_with_an_unknown_field_is_refused(self):
        self.real(needs_human=["ears"])
        self.human_at("ears", who="ana")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertIsNone(rep[0][2])
        self.assertIn("unknown field", rep[0][3])

    def test_imported_evidence_under_human_machine_is_read(self):
        self.real(needs_human=["ears"])
        self.human_at("ears", folder="lenovo-t7")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertEqual(rep[0][2], 100, rep)
        # another machine's folder does not stand in for this machine
        os.remove(os.path.join(self.f.out, "human", "lenovo-t7", "F-T-1.ears.lenovo-t7.json"))
        self.human_at("ears", folder="hp")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertIsNone(rep[0][2])

    def test_a_pass_of_a_team_member_counts_and_the_owner_sample_is_recorded(self):
        picked = next(c for c in ("c%d" % i for i in range(500)) if evidence.sampled_for_owner({"row": "F-T-1", "clause": c, "machine": "lenovo-t7", "image_commit": "abc12345", "by": "admin"}))
        skipped = next(c for c in ("c%d" % i for i in range(500)) if not evidence.sampled_for_owner({"row": "F-T-1", "clause": c, "machine": "lenovo-t7", "image_commit": "abc12345", "by": "admin"}))
        self.real(needs_human=[picked, skipped])
        self.human(picked, by="admin")
        self.human(skipped, by="person")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertEqual(rep[0][2], 100, rep)
        self.assertIn("owner review sampled (10 percent rule): %s (by admin)" % picked, rep[0][3])
        self.assertNotIn(skipped, rep[0][3])
        # the owner's own pass is never sampled; the same pass by admin would be
        self.human(picked, by="owner")
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertNotIn("sampled", rep[0][3])
        self.assertFalse(evidence.sampled_for_owner({"by": "owner", "row": "F-T-1", "clause": picked, "machine": "lenovo-t7", "image_commit": "abc12345"}))

    def test_about_a_tenth_of_the_other_passes_are_sampled_and_the_choice_is_stable(self):
        hits = [evidence.sampled_for_owner({"row": "F-T-1", "clause": "c%d" % i, "machine": "m", "image_commit": "abc1234", "by": "person"}) for i in range(2000)]
        self.assertGreater(sum(hits), 120)
        self.assertLess(sum(hits), 290)
        self.assertEqual(hits, [evidence.sampled_for_owner({"row": "F-T-1", "clause": "c%d" % i, "machine": "m", "image_commit": "abc1234", "by": "person"}) for i in range(2000)])

    def test_the_review_queue_lists_sampled_passes_of_both_folders(self):
        picked = [c for c in ("c%d" % i for i in range(60)) if evidence.sampled_for_owner({"row": "F-T-1", "clause": c, "machine": "lenovo-t7", "image_commit": "abc12345", "by": "person"})]
        self.assertTrue(picked)
        self.human_at(picked[0], by="person")
        if len(picked) > 1:
            self.human_at(picked[1], folder="lenovo-t7", by="admin")
        self.human_at(picked[0] + "x", by="person", verdict="fail")
        q = evidence.review_queue(os.path.join(self.f.out, "human"))
        self.assertEqual(sorted(e["clause"] for e in q), sorted(picked[:2]))
        self.assertEqual(evidence.review_queue(os.path.join(self.f.out, "nothing")), [])

    def test_a_real_result_with_a_not_covered_clause_gives_nothing(self):
        self.real(not_covered=["sound"])
        _n, rep = self.f.run(features(test="70", manual="needed"))
        self.assertIsNone(rep[0][2])


class Cli(unittest.TestCase):
    def test_dry_run_does_not_write_and_apply_does(self):
        f = Fix(self)
        f.add("run1")
        f.add("run2", when="2026-11-22T02:30:00")
        p = os.path.join(f.tmp.name, "FEATURES.md")
        open(p, "w").write(features())
        argv = [f.out, "--features", p, "--now", "2026-11-22T12:00:00", "--image-commit", "abc12345"]
        import io
        import contextlib
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            evidence.main(argv, p, f.repo)
        self.assertEqual(open(p).read(), features())
        self.assertIn("dry run", buf.getvalue())
        with contextlib.redirect_stdout(io.StringIO()):
            evidence.main(argv + ["--apply"], p, f.repo)
        self.assertIn("| 100/70 |", open(p).read())

    def test_the_result_schema_refuses_a_pass_without_evidence_and_an_unknown_status(self):
        r = R.new_result("F-T-1", "s", "qemu", "PASS")
        self.assertTrue(R.validate(r))
        with self.assertRaises(ValueError):
            R.new_result("F-T-1", "s", "qemu", "MAYBE")


if __name__ == "__main__":
    unittest.main()
