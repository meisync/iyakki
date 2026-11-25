"""The pairing code the QEMU harness reads off the screen: only readings the device can print, nearest first, at most the five tries the device allows (Q1, 2026-11-21)."""
import os, sys, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "qemu"))
sys.path.insert(0, os.path.join(HERE, "..", "..", "devtest"))
sys.path.insert(0, os.path.join(HERE, ".."))
import session_lib  # noqa: E402


class PairCode(unittest.TestCase):
    def test_a_clean_reading_comes_first(self):
        self.assertEqual(session_lib._variants("HJYAV-TUFAJ")[0], "HJYAV-TUFAJ")

    def test_characters_the_device_never_prints_are_replaced(self):
        for c in session_lib._variants("HIYAV-TUF0J"):
            self.assertTrue(all(ch in session_lib.CODE_ALPHABET or ch == "-" for ch in c), c)
        self.assertEqual(session_lib._variants("HIYAV-TUFAJ")[0], "HJYAV-TUFAJ")

    def test_at_most_five_tries_and_no_repeats(self):
        v = session_lib._variants("IIIII-IIIII")
        self.assertLessEqual(len(v), 5)
        self.assertEqual(len(v), len(set(v)))

    def test_the_alphabet_is_the_one_of_the_device(self):
        src = open(os.path.join(HERE, "..", "..", "..", "crates", "iyakki-agent", "src", "devmode", "auth.rs")).read()
        self.assertIn('b"%s"' % session_lib.CODE_ALPHABET, src)


if __name__ == "__main__":
    unittest.main()
