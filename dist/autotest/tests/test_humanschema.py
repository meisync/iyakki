import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import humanschema as H  # noqa: E402
import results as R  # noqa: E402


def good_human(**kw):
    h = {"schema": 1, "row": "F-T-1", "clause": "ears-1", "machine": "lenovo", "image_commit": "abc1234", "by": "owner", "verdict": "pass",
         "date": "2026-11-22", "note": "two notes, left then right"}
    h.update(kw)
    return h


def good_list(**kw):
    c = {"schema": 1, "row": "F-T-1", "feature_sha": R.feature_sha("x"), "title": "T",
         "clauses": [{"id": "c1", "text": "t", "machines": ["lenovo"], "steps": [{"do": "press play", "expect": "two notes"}]}]}
    c.update(kw)
    return c


class Human(unittest.TestCase):
    def test_a_good_record_is_accepted_and_written_under_its_name(self):
        self.assertEqual(H.validate_human(good_human()), [])
        with tempfile.TemporaryDirectory() as d:
            p = H.write_human(d, good_human(photo="photos/ears.jpg"))
            self.assertTrue(p.endswith("F-T-1.ears-1.lenovo.json"))
            self.assertEqual(json.load(open(p))["verdict"], "pass")

    def test_each_bad_field_is_refused(self):
        for kw in ({"verdict": "ok"}, {"row": "x"}, {"clause": "a b"}, {"machine": ""}, {"image_commit": "zz"}, {"by": ""}, {"by": "ana"}, {"by": "Owner"}, {"date": "yesterday"},
                   {"schema": 2}, {"photo": "/etc/passwd"}, {"photo": "../x.png"}, {"extra": 1}, {"note": "x" * 3000}):
            self.assertTrue(H.validate_human(good_human(**kw)), kw)
        with tempfile.TemporaryDirectory() as d, self.assertRaises(ValueError):
            H.write_human(d, good_human(verdict="maybe"))

    def test_every_role_is_accepted_and_the_list_is_the_documented_one(self):
        self.assertEqual(H.BY_ROLES, ("owner", "admin", "person"))
        for role in H.BY_ROLES:
            self.assertEqual(H.validate_human(good_human(by=role)), [], role)

    def test_cannot_test_is_a_valid_verdict_and_is_not_a_pass(self):
        self.assertEqual(H.validate_human(good_human(verdict="cannot-test")), [])


class Checklist(unittest.TestCase):
    def test_a_good_checklist(self):
        self.assertEqual(H.validate_checklist(good_list()), [])

    def test_bad_checklists(self):
        bad = [good_list(row="nope"), good_list(feature_sha="1"), good_list(clauses=[]), good_list(title=""),
               good_list(clauses=[{"id": "c1", "text": "t", "machines": ["toaster"], "steps": [{"do": "a", "expect": "b"}]}]),
               good_list(clauses=[{"id": "c1", "text": "t", "machines": ["hp"], "steps": []}]),
               good_list(clauses=[{"id": "c1", "text": "t", "machines": ["hp"], "steps": [{"do": "a"}]}]),
               good_list(clauses=[{"id": "c1", "text": "t", "machines": ["hp"], "steps": [{"do": "a", "expect": "b", "screenshot": "/abs.png"}]}]),
               good_list(clauses=[{"id": "c1", "text": "t", "machines": ["hp"], "steps": [{"do": "a", "expect": "b"}]}] * 2)]
        for c in bad:
            self.assertTrue(H.validate_checklist(c), c)

    def test_the_shipped_checklists_are_valid(self):
        for p, c, bad in H.load_checklists(os.path.join(HERE, "..", "checklists")):
            self.assertEqual(bad, [], p)


if __name__ == "__main__":
    unittest.main()
