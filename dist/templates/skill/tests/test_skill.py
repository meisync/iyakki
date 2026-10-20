"""Checks that skill.md is a valid IYAKKI skill: front matter, matching name, size, plain ASCII."""
import json
import os
import re
import unittest

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load():
    with open(os.path.join(HERE, "skill.md"), encoding="utf-8") as f:
        text = f.read()
    m = re.match(r"---\n(.*?)\n---\n(.*)", text, re.S)
    assert m, "the skill starts with a --- header"
    head = dict(line.split(": ", 1) for line in m.group(1).splitlines() if ": " in line)
    return text, head, m.group(2)


class SkillTest(unittest.TestCase):
    def test_front_matter(self):
        _, head, _ = load()
        self.assertTrue(head.get("name") and head.get("description"))
        self.assertLessEqual(len([t for t in head.get("triggers", "").split(",") if t.strip()]), 12)

    def test_name_matches_project(self):
        _, head, _ = load()
        with open(os.path.join(HERE, "iyakki-app.json"), encoding="utf-8") as f:
            self.assertEqual(head["name"], json.load(f)["name"])
        self.assertRegex(head["name"], r"^[a-z0-9][a-z0-9-]{0,47}$")

    def test_size_and_ascii(self):
        text, _, body = load()
        self.assertLessEqual(len(body), 3950)
        text.encode("ascii")

    def test_no_secrets(self):
        text, _, _ = load()
        self.assertNotRegex(text, r"(ghp_|sk-ant-|AKIA|-----BEGIN)")


if __name__ == "__main__":
    unittest.main()
