import json
import os
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import checklist  # noqa: E402
import humanschema as H  # noqa: E402
from test_humanschema import good_human, good_list  # noqa: E402


def make_root(tc, row="F-T-1", title="Tone <b>test</b>"):
    tmp = tempfile.TemporaryDirectory()
    tc.addCleanup(tmp.cleanup)
    cl = os.path.join(tmp.name, "lists")
    os.makedirs(cl)
    c = good_list(row=row, title=title)
    c["clauses"].append({"id": "c2", "text": "hp only", "machines": ["hp"], "steps": [{"do": "d", "expect": "e"}]})
    with open(os.path.join(cl, row + ".json"), "w") as fh:
        json.dump(c, fh)
    return tmp.name, cl


class Pages(unittest.TestCase):
    def test_one_page_per_machine_with_only_its_clauses_and_html_is_escaped(self):
        root, cl = make_root(self)
        paths = checklist.write_pages(root, os.path.join(root, "checklist"), cl)
        names = sorted(os.path.basename(p) for p in paths)
        self.assertEqual(names, ["hp.html", "index.html", "lenovo.html"])
        lenovo = open(os.path.join(root, "checklist", "lenovo.html")).read()
        hp = open(os.path.join(root, "checklist", "hp.html")).read()
        self.assertIn('data-clause="c1"', lenovo)
        self.assertNotIn('data-clause="c2"', lenovo)
        self.assertIn('data-clause="c2"', hp)
        self.assertIn("Tone &lt;b&gt;test&lt;/b&gt;", lenovo)
        for word in ("Pass", "Fail", "Cannot test"):
            self.assertIn(word, lenovo)
        self.assertNotIn("password", lenovo.lower())

    def test_a_bad_checklist_file_is_left_out(self):
        root, cl = make_root(self)
        with open(os.path.join(cl, "F-T-2.json"), "w") as fh:
            json.dump({"schema": 1}, fh)
        p = checklist.write_pages(root, os.path.join(root, "checklist"), cl)
        self.assertNotIn("F-T-2", open(p[0]).read())


class Server(unittest.TestCase):
    def start(self):
        root, cl = make_root(self)
        checklist.write_pages(root, os.path.join(root, "checklist"), cl)
        s = checklist.serve(root, 0)
        th = threading.Thread(target=s.serve_forever, daemon=True)
        th.start()
        self.addCleanup(lambda: (s.shutdown(), s.server_close(), th.join()))
        return root, "http://127.0.0.1:%d" % s.server_address[1]

    def post(self, base, body):
        req = urllib.request.Request(base + "/save", data=body if isinstance(body, bytes) else json.dumps(body).encode(), method="POST")
        try:
            with urllib.request.urlopen(req, timeout=10) as r:
                return r.status, r.read().decode()
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode()

    def test_the_page_is_served_and_an_answer_is_written_under_its_contract_name(self):
        root, base = self.start()
        with urllib.request.urlopen(base + "/lenovo.html", timeout=10) as r:
            self.assertIn(b"Checklist: lenovo", r.read())
        code, name = self.post(base, good_human())
        self.assertEqual((code, name), (200, "F-T-1.ears-1.lenovo.json"))
        self.assertEqual(H.validate_human(json.load(open(os.path.join(root, "human", name)))), [])

    def test_bad_answers_and_paths_are_refused(self):
        root, base = self.start()
        self.assertEqual(self.post(base, good_human(verdict="great"))[0], 400)
        self.assertEqual(self.post(base, b"not json")[0], 400)
        self.assertEqual(self.post(base, b"x" * 70000)[0], 413)
        self.assertEqual(self.post(base, good_human(photo="/etc/shadow"))[0], 400)
        for path in ("/../../etc/passwd", "/a/b.html", "/human"):
            with self.assertRaises(urllib.error.HTTPError):
                urllib.request.urlopen(base + path, timeout=10)
        self.assertFalse(os.path.exists(os.path.join(root, "human")))

    def test_the_server_listens_on_loopback_only(self):
        root, cl = make_root(self)
        s = checklist.serve(root, 0)
        self.addCleanup(s.server_close)
        self.assertEqual(s.server_address[0], "127.0.0.1")


if __name__ == "__main__":
    unittest.main()
