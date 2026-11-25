"""heard_wait: a new line is waited for by looks, a line that was already there never counts (F-SDK-203 and F-SDK-197 checks)."""
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "qemu"))
import heard_wait as H  # noqa: E402


class Screen:
    def __init__(self, frames):
        self.frames, self.n, self.pauses = frames, 0, 0

    def read(self):
        t = self.frames[min(self.n, len(self.frames) - 1)]
        self.n += 1
        return t

    def pause(self):
        self.pauses += 1


class HeardWaitTests(unittest.TestCase):
    def test_a_line_that_was_there_before_does_not_count(self):
        s = Screen(["usb added mouse A\n"])
        self.assertEqual(H.count(s.read(), r"usb added [^\n]*"), 1)
        with self.assertRaises(AssertionError):
            H.wait_new(s.read, r"usb added [^\n]*", 1, looks=4, pause=s.pause)

    def test_the_new_line_is_found_on_the_look_it_appears(self):
        s = Screen(["nothing\n", "nothing\n", "usb added mouse A\nusb added mouse B\n"])
        got = H.wait_new(s.read, r"usb added [^\n]*", 1, looks=10, pause=s.pause)
        self.assertEqual(got, "usb added mouse B")
        self.assertEqual(s.n, 3)
        self.assertEqual(s.pauses, 2)

    def test_the_guard_is_a_number_of_looks_and_the_failure_shows_the_tail(self):
        s = Screen(["Heard log\nempty"])
        with self.assertRaises(AssertionError) as cm:
            H.wait_new(s.read, r"usb removed [^\n]*", 0, looks=3, pause=s.pause)
        self.assertEqual(s.n, 3)
        self.assertEqual(s.pauses, 2)
        self.assertIn("Heard log | empty", str(cm.exception))


if __name__ == "__main__":
    unittest.main()
