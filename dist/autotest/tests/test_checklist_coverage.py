"""Every scenario that waits for a person (a `human:` header) has a checklist file for the Doctor Manual tests tab: it validates, names the
same row, carries every `human:` clause id and the same feature hash as the scenario (lane Q8). The Rust side parses the same folder
(manualtest.rs, the_checklists_of_the_repository_parse) and post-build.sh installs it in the image."""
import glob
import json
import os
import re
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import humanschema as H  # noqa: E402

SCN = os.path.join(HERE, "..", "scenarios")
CHK = os.path.join(HERE, "..", "checklists")
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))


class ChecklistCoverage(unittest.TestCase):
    def scenarios_with_a_person(self):
        out = []
        for f in sorted(glob.glob(os.path.join(SCN, "*.scn"))):
            t = open(f).read()
            clauses = re.findall(r"^human:\s*([A-Za-z0-9_-]+)\s*\|", t, re.M)
            if clauses:
                sha = re.search(r"^feature-sha:\s*(\w+)", t, re.M)
                out.append((os.path.basename(f)[:-4], clauses, sha.group(1) if sha else ""))
        return out

    def test_there_are_scenarios_that_wait_for_a_person(self):
        self.assertGreater(len(self.scenarios_with_a_person()), 10)

    def test_each_has_a_valid_checklist_with_its_clauses_and_hash(self):
        for row, clauses, sha in self.scenarios_with_a_person():
            p = os.path.join(CHK, row + ".json")
            self.assertTrue(os.path.exists(p), "no checklist for %s" % row)
            c = json.load(open(p))
            self.assertEqual(H.validate_checklist(c), [], row)
            self.assertEqual(c["row"], row)
            self.assertEqual(c["feature_sha"], sha, "%s: the checklist and the scenario name different Feature hashes" % row)
            ids = {x["id"] for x in c["clauses"]}
            for cl in clauses:
                self.assertIn(cl, ids, "%s: clause %s is not in the checklist" % (row, cl))

    def test_post_build_installs_the_checklists_for_the_doctor_tab(self):
        t = open(os.path.join(ROOT, "dist", "buildroot", "board", "iyakki", "post-build.sh")).read()
        self.assertIn("autotest/checklists", t)
        self.assertIn("usr/share/iyakki/manual/checklists", t)


if __name__ == "__main__":
    unittest.main()
