import unittest
import recipes_loss_parse as P


class Status(unittest.TestCase):
    def test_states_are_read(self):
        self.assertEqual(P.drive_state("drive: Healthy (since uptime 120 s), device /dev/sda2, UUID abc\n"), ("Healthy", "/dev/sda2"))
        self.assertEqual(P.drive_state("drive: Lost (since uptime 130 s), device /dev/sda2, UUID abc\nreason: unplugged\n"), ("Lost", "/dev/sda2"))
        self.assertEqual(P.drive_state("drive: Reattachable (since uptime 140 s), device /dev/sdb2, UUID abc\n"), ("Reattachable", "/dev/sdb2"))
        self.assertEqual(P.drive_state("drive: not watched (the data is in RAM, or the watcher is not running)\n"), ("NotWatched", ""))
        self.assertIsNone(P.drive_state("nonsense"))
        self.assertIsNone(P.drive_state("drive: Sleeping (since uptime 1 s), device /dev/sda2"))


def s(*rows):
    return [(t, w, d) for t, w, d in rows]


class Judge(unittest.TestCase):
    def test_the_full_path_passes(self):
        v, why, f = P.judge(s((0, "Healthy", "/dev/sda2"), (4, "Lost", "/dev/sda2"), (9, "Reattachable", "/dev/sdb2"), (14, "Healthy", "/dev/sdb2")))
        self.assertEqual(v, "PASS", why)
        self.assertEqual((f["device_before"], f["device_after"]), ("/dev/sda2", "/dev/sdb2"))
        self.assertEqual(f["first_lost_s"], 4)

    def test_stuck_states_fail(self):
        self.assertEqual(P.judge(s((0, "Healthy", "a"), (3, "Lost", "a"), (60, "Lost", "a")))[0], "FAIL")
        self.assertEqual(P.judge(s((0, "Healthy", "a"), (3, "Lost", "a"), (9, "Reattachable", "b"), (60, "Reattachable", "b")))[0], "FAIL")
        self.assertEqual(P.judge(s((0, "Healthy", "a"), (3, "Lost", "a"), (9, "Failed", "")))[0], "FAIL")

    def test_no_unplug_or_unusable_start_is_not_a_failure(self):
        self.assertEqual(P.judge(s((0, "Healthy", "a"), (60, "Healthy", "a")))[0], "NEEDS-HUMAN")
        self.assertEqual(P.judge(s((0, "NotWatched", "")))[0], "NEEDS-HUMAN")
        self.assertEqual(P.judge(s((0, "Lost", "a")))[0], "NEEDS-HUMAN")
        self.assertEqual(P.judge([])[0], "NEEDS-HUMAN")

    def test_an_unanswered_status_counts_as_lost(self):
        v, _, _ = P.judge(s((0, "Healthy", "a"), (5, "Lost", ""), (12, "Reattachable", "b"), (20, "Healthy", "b")))
        self.assertEqual(v, "PASS")


if __name__ == "__main__":
    unittest.main()
