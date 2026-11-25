"""The sound verbs of the scenario format: they parse, the runner knows when to record, and a scenario using them is refused without a recording.
The WAVs are synthesised (test_listen.make_wav), written to a temporary folder and removed."""
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
sys.path.insert(0, HERE)
import builtin_scenarios  # noqa: E402
import devui  # noqa: E402
import run  # noqa: E402
import verbs  # noqa: E402
from test_listen import make_wav, silence, tone  # noqa: E402

SOUND = ["expect-tone 440", "expect-tone 880 25 left", "expect-level -30 -6", "expect-silence", "expect-sound-within 2", "expect-speech 1 4"]


class FakeT:
    """The part of devui.Test the sound verbs use: check() records, note() records."""

    def __init__(self):
        self.checks, self.notes = [], []

    def check(self, name, ok, detail=""):
        self.checks.append((name, bool(ok), detail))
        if not ok:
            raise devui.CheckFailed("%s: %s" % (name, detail))

    def note(self, text):
        self.notes.append(text)


class Parsing(unittest.TestCase):
    def test_every_sound_verb_is_registered_as_a_scenario_verb(self):
        for v in verbs.SOUND_VERBS:
            self.assertIn(v, builtin_scenarios.EXTRA_VERBS, v)

    def test_a_scenario_with_sound_verbs_parses_into_steps(self):
        text = "row: F-SND-1\nclause: 1 the tone plays\ntargets: qemu\n" + "\n".join(SOUND) + "\n"
        head, steps = builtin_scenarios.parse_scn(text)
        self.assertEqual(steps, SOUND)
        self.assertEqual(head["targets"], "qemu")
        self.assertTrue(verbs.uses_sound(steps))

    def test_a_scenario_without_sound_verbs_does_not_ask_for_a_recording(self):
        self.assertFalse(verbs.uses_sound(["key Tab", "expect-text IYAKKI", "expect-silence-ish 1"]))
        self.assertFalse(verbs.uses_sound(["expect-tone-ish 440"]), "a verb is matched by its whole name")

    def test_a_sound_scenario_on_a_non_qemu_target_is_a_format_problem(self):
        entry = {"file": "F-SND-9.scn", "rows": ["F-SND-9"], "meta": {"clause": "c"}, "targets": ["qemu", "browser"], "steps": ["expect-tone 440"]}
        bad = run.problems([entry])
        self.assertTrue(any("targets: qemu" in b for b in bad), bad)

    def test_a_sound_scenario_on_qemu_alone_is_no_problem(self):
        entry = {"file": "F-SND-9.scn", "rows": ["F-SND-9"], "meta": {"clause": "c"}, "targets": ["qemu"], "steps": ["expect-tone 440"]}
        self.assertEqual(run.problems([entry]), [])


class Refusal(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.addCleanup(verbs.Active.reset)

    def scenario(self, name, lines):
        path = os.path.join(self.tmp.name, name + ".scn")
        with open(path, "w") as fh:
            fh.write("row: F-SND-8\ntargets: qemu\nclause: 1 sound\n" + "\n".join(lines) + "\n")
        builtin_scenarios._register_text(path)
        return devui.SCENARIOS[name]["fn"]

    def test_a_sound_scenario_is_refused_when_no_recording_exists(self):
        fn = self.scenario("snd-refused-none", ["expect-tone 440"])
        verbs.Active.reset("F-SND-8")  # no wav_path: this run records nothing
        t = FakeT()
        with self.assertRaises(devui.CheckFailed) as cm:
            fn(t)
        self.assertIn("refused", str(cm.exception))
        self.assertIn("-audiodev wav", str(cm.exception))
        self.assertEqual(t.checks, [], "nothing is passed")

    def test_a_recording_path_that_is_missing_is_refused_with_the_path(self):
        missing = os.path.join(self.tmp.name, "guest.wav")
        fn = self.scenario("snd-refused-missing", ["expect-silence"])
        verbs.Active.reset("F-SND-8", wav_path=missing)
        with self.assertRaisesRegex(devui.CheckFailed, "no recording at"):
            fn(FakeT())

    def test_a_scenario_with_a_recording_judges_only_the_frames_after_it_began(self):
        path = os.path.join(self.tmp.name, "guest.wav")
        with open(path, "wb") as fh:
            fh.write(make_wav([silence(1.0)]))  # the recording before the scenario: silence
        verbs.Active.reset("F-SND-8", wav_path=path)  # the window starts after this second
        with open(path, "wb") as fh:
            fh.write(make_wav([silence(1.0) + tone(440, 1.0), silence(2.0)]))  # the scenario's own second: a tone on the left
        fn = self.scenario("snd-window", ["expect-tone 440 10 left", "expect-silence"])
        t = FakeT()
        with self.assertRaises(devui.CheckFailed):
            fn(t)  # the tone is there; the silence check then fails
        self.assertEqual([c[1] for c in t.checks], [True, False])

    def test_the_verbs_pass_and_fail_on_the_right_recording(self):
        path = os.path.join(self.tmp.name, "guest.wav")
        with open(path, "wb") as fh:
            fh.write(make_wav([silence(0.1), silence(0.1)]))  # what was there before the scenario began
        verbs.Active.reset("F-SND-8", wav_path=path)
        with open(path, "wb") as fh:
            fh.write(make_wav([tone(440, 1.0, amp=0.5), silence(1.0)]))  # the scenario's audio
        fn = self.scenario("snd-pass", ["expect-tone 440 10 left", "expect-level -18 -12", "expect-sound-within 0.5", "expect-speech 0.5 3"])
        t = FakeT()
        fn(t)
        self.assertEqual([c[1] for c in t.checks], [True, True, True, True])

    def test_a_tone_check_names_the_channel_and_the_frequency(self):
        path = os.path.join(self.tmp.name, "guest.wav")
        with open(path, "wb") as fh:
            fh.write(make_wav([silence(1.0), tone(440, 1.0)]))
        verbs.Active.reset("F-SND-8", wav_path=path)
        fn = self.scenario("snd-names", ["expect-tone 440 10 left"])
        t = FakeT()
        with self.assertRaises(devui.CheckFailed):
            fn(t)
        self.assertIn("440", t.checks[0][0] if t.checks else "")
        self.assertIn("left", t.checks[0][0] if t.checks else "")


if __name__ == "__main__":
    unittest.main()
