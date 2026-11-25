#!/usr/bin/env python3
"""Tests of dist/site/gen-known-limits.py: python3 dist/site/test_known_limits.py (no network, no sleeps)."""
import importlib.util, os, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("gen_known_limits", os.path.join(HERE, "gen-known-limits.py"))
gen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen)


class Headline(unittest.TestCase):
    def test_cuts_at_the_first_colon_and_drops_links_and_code(self):
        self.assertEqual(gen.headline("Secure Boot on real hardware: shim + MOK ([docs/SECURE-BOOT.md](docs/SECURE-BOOT.md)) is proven"), "Secure Boot on real hardware")

    def test_drops_parentheses_and_ids(self):
        self.assertEqual(gen.headline("The `hw` image (firmware blobs) was never booted on a real machine"), "The hw image was never booted on a real machine")

    def test_long_text_is_cut_at_a_word(self):
        h = gen.headline("word " * 60)
        self.assertTrue(h.endswith("...") or len(h) <= 110)
        self.assertLessEqual(len(h), 110)

    def test_pipes_cannot_break_the_table(self):
        self.assertNotIn("|", gen.headline("a | b is limited here and that is all there is to say about it"))


class Source(unittest.TestCase):
    def test_the_real_source_builds_without_errors_and_is_fresh(self):
        text, errors = gen.build()
        self.assertEqual(errors, [])
        with open(gen.OUT, encoding="utf-8") as f:
            self.assertEqual(f.read(), text, "run python3 dist/site/gen-known-limits.py")

    def test_the_prose_carries_no_internal_ids(self):
        text, _ = gen.build()
        prose = text.split("## The full list")[0]
        self.assertIsNone(gen.ID.search(prose), gen.ID.search(prose))

    def test_every_limit_and_unproven_entry_is_in_the_table(self):
        text, _ = gen.build()
        for k, (c, _, _) in gen.known_issues().items():
            if c in ("LIMIT", "UNPROVEN"):
                self.assertIn("| %s |" % k, text)

    def test_every_experimental_row_exists(self):
        fr = gen.features()
        sections, errors = gen.parse_source()
        self.assertEqual(errors, [])
        titles = [t for t, _ in sections]
        self.assertIn("Experimental: switched off unless you turn it on, and labelled in the product", titles)
        for t, items in sections:
            for _, ids in items:
                for i in ids:
                    if i.startswith("F-"):
                        self.assertIn(i, fr)


if __name__ == "__main__":
    unittest.main()
