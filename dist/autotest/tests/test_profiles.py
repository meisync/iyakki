"""The QEMU boot profiles are written twice (dist/autotest/run.py and dist/qemu/session-checks.py): both must name every profile a scenario or a session check asks for."""
import glob
import os
import re
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
sys.path.insert(0, os.path.join(HERE, ".."))
import run  # noqa: E402


def profiles_of_session_checks_py():
    text = open(os.path.join(ROOT, "dist", "qemu", "session-checks.py")).read()
    block = text[text.index("PROFILES = {"):]
    block = block[:block.index("results, bad")]
    return set(re.findall(r'"(\w+)": \{', block))


class ProfileTests(unittest.TestCase):
    def test_every_scenario_profile_is_a_known_boot_profile(self):
        for f in sorted(glob.glob(os.path.join(HERE, "..", "scenarios", "*.scn"))):
            for line in open(f):
                if line.startswith("profile:"):
                    self.assertIn(line.split(":", 1)[1].strip(), run.PROFILES, os.path.basename(f))

    def test_every_profile_of_the_session_check_list_is_known_to_both_runners(self):
        listed = set(re.findall(r"^profile:\s*(\w+)", open(os.path.join(ROOT, "dist", "qemu", "session-checks.list")).read(), re.M))
        self.assertTrue(listed)
        self.assertLessEqual(listed, set(run.PROFILES))
        self.assertLessEqual(listed, profiles_of_session_checks_py())

    def test_both_runners_define_the_same_profiles(self):
        self.assertEqual(set(run.PROFILES), profiles_of_session_checks_py())

    def test_the_usb_profile_starts_the_controller_at_boot(self):
        self.assertIn("qemu-xhci,id=evxhci", run.PROFILES["usb"]["extra_qemu"])


if __name__ == "__main__":
    unittest.main()
