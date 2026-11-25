"""The in-house WAV listener (dist/autotest/listen.py) on WAVs synthesised here: nothing is read from disk, nothing is committed."""
import math
import os
import random
import struct
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import listen  # noqa: E402


def _quantise(v, bits):
    half = 2 ** (bits - 1)
    return max(-half, min(half - 1, int(round(v * half))))


def make_wav(channels, rate=48000, bits=16, header_size=None):
    """channels: a list of equal-length lists of floats (-1..1). Returns the bytes of a PCM WAV file."""
    n = len(channels[0])
    body = bytearray()
    for i in range(n):
        for ch in channels:
            q = _quantise(ch[i], bits)
            if bits == 8:
                body += bytes([q + 128])
            elif bits == 16:
                body += struct.pack("<h", q)
            else:
                body += q.to_bytes(3, "little", signed=True)
    align = len(channels) * bits // 8
    fmt = struct.pack("<HHIIHH", 1, len(channels), rate, rate * align, align, bits)
    data_size = len(body) if header_size is None else header_size
    riff = 4 + (8 + len(fmt)) + (8 + len(body))
    return b"RIFF" + struct.pack("<I", riff) + b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", data_size) + bytes(body)


def tone(hz, seconds, amp=0.5, rate=48000):
    return [amp * math.sin(2 * math.pi * hz * i / rate) for i in range(int(seconds * rate))]


def silence(seconds, rate=48000):
    return [0.0] * int(seconds * rate)


def noise(seconds, amp, seed, rate=48000):
    rnd = random.Random(seed)
    return [amp * (2 * rnd.random() - 1) for _ in range(int(seconds * rate))]


def bursts(pattern, rate=48000, amp=0.2, seed=7):
    """pattern: [(kind, seconds)], kind 'n' = a noise burst (speech-like), 's' = silence."""
    out = []
    for i, (kind, sec) in enumerate(pattern):
        out += noise(sec, amp, seed + i, rate) if kind == "n" else silence(sec, rate)
    return out


class Tones(unittest.TestCase):
    def test_a_440_tone_is_present_at_its_frequency_and_level(self):
        wav = listen.load(make_wav([tone(440, 1.0, amp=0.5)]))
        r = listen.tone_present(wav, 440)
        self.assertTrue(r["present"])
        self.assertAlmostEqual(r["hz"], 440, delta=1)
        self.assertAlmostEqual(r["level_dbfs"], -9.03, delta=0.5)  # sine of amplitude 0.5 has RMS 0.354

    def test_a_two_note_sequence_has_each_note_where_it_is(self):
        wav = listen.load(make_wav([tone(440, 1.0) + tone(880, 1.0)]))
        low = listen.tone_present(wav, 440)
        high = listen.tone_present(wav, 880)
        self.assertTrue(low["present"] and high["present"])
        self.assertLess(low["first_at"], 0.15)
        self.assertLess(low["last_at"], 1.15)
        self.assertGreater(high["first_at"], 0.85)
        self.assertFalse(listen.tone_present(wav, 660)["present"])

    def test_a_tone_at_another_frequency_is_not_present(self):
        wav = listen.load(make_wav([tone(440, 1.0)]))
        self.assertFalse(listen.tone_present(wav, 1000)["present"])

    def test_the_tolerance_takes_a_tone_a_little_off(self):
        wav = listen.load(make_wav([tone(445, 1.0)]))
        self.assertTrue(listen.tone_present(wav, 440, tol_hz=10)["present"])

    def test_a_quiet_tone_below_min_dbfs_is_not_present(self):
        wav = listen.load(make_wav([tone(440, 1.0, amp=0.001)]))
        self.assertFalse(listen.tone_present(wav, 440, min_dbfs=-40)["present"])

    def test_a_short_tone_below_min_seconds_is_not_present(self):
        wav = listen.load(make_wav([tone(440, 0.1) + silence(0.9)]))
        self.assertFalse(listen.tone_present(wav, 440, min_seconds=0.5)["present"])

    def test_the_left_only_tone_is_found_on_the_left_and_not_on_the_right(self):
        wav = listen.load(make_wav([tone(440, 1.0), silence(1.0)]))
        self.assertTrue(listen.tone_present(wav, 440, channel="left")["present"])
        self.assertFalse(listen.tone_present(wav, 440, channel="right")["present"])
        self.assertTrue(listen.tone_present(wav, 440)["present"], "the mix still carries it")

    def test_right_only_tone_is_found_on_the_right(self):
        wav = listen.load(make_wav([silence(1.0), tone(440, 1.0)]))
        self.assertTrue(listen.tone_present(wav, 440, channel="right")["present"])
        self.assertFalse(listen.tone_present(wav, 440, channel="left")["present"])


class Levels(unittest.TestCase):
    def test_a_sine_of_amplitude_half_has_the_expected_rms_and_peak(self):
        lv = listen.level_dbfs(listen.load(make_wav([tone(440, 1.0, amp=0.5)])))
        self.assertAlmostEqual(lv["rms_dbfs"], -9.03, delta=0.1)
        self.assertAlmostEqual(lv["peak_dbfs"], -6.02, delta=0.1)

    def test_silence_is_minus_infinity(self):
        lv = listen.level_dbfs(listen.load(make_wav([silence(0.5)])))
        self.assertEqual(lv["rms_dbfs"], -math.inf)

    def test_silence_detection_and_threshold(self):
        self.assertTrue(listen.silence(listen.load(make_wav([silence(1.0)]))))
        self.assertFalse(listen.silence(listen.load(make_wav([tone(440, 1.0, amp=0.01)]))), "-43 dBFS is above -60")
        self.assertTrue(listen.silence(listen.load(make_wav([tone(440, 1.0, amp=0.01)])), threshold_dbfs=-30))

    def test_silence_of_one_channel_is_not_silence_of_the_file(self):
        wav = listen.load(make_wav([tone(440, 1.0), silence(1.0)]))
        self.assertFalse(listen.silence(wav))
        self.assertTrue(listen.silence(wav, channel="right"))

    def test_sound_starts_within_the_given_seconds(self):
        wav = listen.load(make_wav([silence(0.5) + tone(440, 1.0)]))
        self.assertAlmostEqual(listen.first_sound_at(wav), 0.5, delta=0.03)
        self.assertTrue(listen.sound_starts_within(wav, 0.6))
        self.assertFalse(listen.sound_starts_within(wav, 0.3))

    def test_no_sound_at_all_has_no_start(self):
        wav = listen.load(make_wav([silence(1.0)]))
        self.assertIsNone(listen.first_sound_at(wav))
        self.assertFalse(listen.sound_starts_within(wav, 5))

    def test_duration(self):
        self.assertAlmostEqual(listen.duration(listen.load(make_wav([silence(1.5, rate=22050)], rate=22050))), 1.5, places=3)


class Damage(unittest.TestCase):
    def test_a_full_scale_square_wave_clips(self):
        loud = [1.6 * math.copysign(1, v) for v in tone(200, 0.5, amp=1.0)]  # pushed past full scale
        self.assertGreater(listen.clipping(listen.load(make_wav([loud]))), 0)

    def test_a_clean_sine_does_not_clip(self):
        self.assertEqual(listen.clipping(listen.load(make_wav([tone(440, 0.5, amp=0.9)]))), 0)

    def test_balance_says_which_channel_is_louder(self):
        left_only = listen.load(make_wav([tone(440, 1.0), silence(1.0)]))
        self.assertLess(listen.left_right_balance(left_only), -40)
        self.assertIsNone(listen.left_right_balance(listen.load(make_wav([tone(440, 1.0)]))))
        same = listen.load(make_wav([tone(440, 1.0), tone(440, 1.0)]))
        self.assertAlmostEqual(listen.left_right_balance(same), 0.0, places=6)


class Formats(unittest.TestCase):
    def test_24_bit_and_8_bit_read_at_the_same_level(self):
        for bits, delta in ((24, 0.05), (8, 1.0)):
            with self.subTest(bits=bits):
                wav = listen.load(make_wav([tone(440, 1.0, amp=0.5)], bits=bits))
                self.assertEqual(wav.bits, bits)
                self.assertAlmostEqual(listen.level_dbfs(wav)["rms_dbfs"], -9.03, delta=delta)
                self.assertTrue(listen.tone_present(wav, 440)["present"])

    def test_any_sample_rate_is_read(self):
        for rate in (8000, 22050, 44100, 96000):
            with self.subTest(rate=rate):
                wav = listen.load(make_wav([tone(440, 1.0, rate=rate)], rate=rate))
                self.assertEqual(wav.rate, rate)
                self.assertTrue(listen.tone_present(wav, 440)["present"])

    def test_a_recording_with_an_unfinished_header_is_read_to_the_end(self):
        wav_bytes = make_wav([tone(440, 1.0)], header_size=0)
        wav = listen.load(wav_bytes)
        self.assertAlmostEqual(listen.duration(wav), 1.0, places=3)
        self.assertEqual(listen.frame_count(wav_bytes), 48000)

    def test_start_frame_skips_what_came_before_the_scenario(self):
        raw = make_wav([silence(1.0) + tone(440, 1.0)])
        self.assertAlmostEqual(listen.duration(listen.load(raw, start_frame=48000)), 1.0, places=3)
        self.assertFalse(listen.tone_present(listen.load(raw, start_frame=0), 440, min_seconds=1.5)["present"])

    def test_something_that_is_not_a_wav_is_refused(self):
        with self.assertRaises(listen.WavError):
            listen.load(b"not a wave file at all")

    def test_a_float_wav_is_refused_with_the_reason(self):
        raw = bytearray(make_wav([silence(0.1)]))
        raw[20:22] = struct.pack("<H", 3)  # format tag 3: IEEE float
        with self.assertRaisesRegex(listen.WavError, "only PCM"):
            listen.load(bytes(raw))


class Speech(unittest.TestCase):
    def test_a_train_of_bursts_with_short_gaps_is_one_speech_like_span(self):
        # four 0.3 s bursts with 0.2 s gaps: the gaps are under the hangover, the span is 1.8 s
        wav = listen.load(make_wav([silence(0.5) + bursts([("n", 0.3), ("s", 0.2), ("n", 0.3), ("s", 0.2), ("n", 0.3), ("s", 0.2), ("n", 0.3)]) + silence(0.5)]))
        r = listen.speech_like(wav, 1.0, 3.0)
        self.assertTrue(r["ok"])
        self.assertAlmostEqual(r["seconds"], 1.8, delta=0.1)
        self.assertEqual(r["bursts"], 1)

    def test_bursts_with_long_gaps_are_counted_apart(self):
        wav = listen.load(make_wav([bursts([("n", 0.3), ("s", 0.6), ("n", 0.3), ("s", 0.6), ("n", 0.3)])]))
        self.assertEqual(listen.speech_like(wav, 0, 10)["bursts"], 3)

    def test_a_steady_tone_is_measured_and_is_too_long_to_be_speech(self):
        wav = listen.load(make_wav([tone(440, 5.0)]))
        self.assertFalse(listen.speech_like(wav, 1.0, 3.0)["ok"])
        self.assertAlmostEqual(listen.speech_like(wav, 1.0, 3.0)["seconds"], 5.0, delta=0.05)
        self.assertTrue(listen.speech_like(listen.load(make_wav([tone(440, 1.0)])), 0.5, 3.0)["ok"])

    def test_silence_is_not_speech(self):
        wav = listen.load(make_wav([silence(3.0)]))
        self.assertFalse(listen.speech_like(wav, 0.5, 5.0)["ok"])

    def test_a_short_blip_is_not_speech(self):
        wav = listen.load(make_wav([silence(1.0) + noise(0.1, 0.2, 3) + silence(1.0)]))
        self.assertFalse(listen.speech_like(wav, 1.0, 3.0)["ok"])


if __name__ == "__main__":
    unittest.main()
