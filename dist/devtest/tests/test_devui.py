import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import devui  # noqa: E402
import builtin_scenarios  # noqa: E402,F401


def st(app="Home", page="", focused="", secret=False, lines=(), buttons=(), cut=False):
    return devui.State({"app": app, "page": page, "focused": focused, "focused_secret": secret, "cut": cut,
                        "screen": [2560, 1600], "lines": [{"text": t, "x": x, "y": y} for x, y, t in lines],
                        "buttons": [{"label": l, "x": x, "y": y, "w": w, "h": h} for l, x, y, w, h in buttons]})


class GuardTests(unittest.TestCase):
    def test_home_refuses_sign_out_and_shortcuts(self):
        home = st("Home")
        for k in ("esc", "Esc", "escape", "q", "Q", "o", "p", "r", "ctrl+win+q", "super+l", "power"):
            self.assertIsNotNone(devui.key_refusal(k, home), k)

    def test_home_allows_navigation_and_settings(self):
        home = st("Home")
        for k in ("s", "left", "right", "Tab", "PageDown", "w", "W"):
            self.assertIsNone(devui.key_refusal(k, home), k)

    def test_esc_fine_in_settings_but_power_never(self):
        settings = st("Settings", "Display")
        self.assertIsNone(devui.key_refusal("esc", settings))
        self.assertIsNone(devui.key_refusal("ctrl+p", settings))
        self.assertIsNotNone(devui.key_refusal("power", settings))
        self.assertIsNotNone(devui.key_refusal("ctrl+win+q", settings))

    def test_unknown_state_sends_nothing(self):
        self.assertIsNotNone(devui.key_refusal("down", None))
        self.assertIsNotNone(devui.text_refusal("abc", None))

    def test_signed_out_screens_refuse_everything(self):
        so = st("Sign in")
        self.assertIsNotNone(devui.key_refusal("down", so))
        self.assertIsNotNone(devui.text_refusal("x", so))
        self.assertIsNotNone(devui.tap_refusal("OK", so))

    def test_text_rules(self):
        self.assertIsNotNone(devui.text_refusal("abc", st("Home")))
        self.assertIsNotNone(devui.text_refusal("abc", st("Settings", focused="")))
        self.assertIsNotNone(devui.text_refusal("abc", st("Settings", focused="Search", secret=True)))
        self.assertIsNone(devui.text_refusal("abc", st("Settings", focused="Search")))

    def test_secret_focus_refuses_keys(self):
        self.assertIsNotNone(devui.key_refusal("a", st("Settings", focused="pw", secret=True)))

    def test_dangerous_tap_labels(self):
        for l in ("Power", "Sign out", "Restart", "Delete chat", "Reboot now"):
            self.assertIsNotNone(devui.tap_refusal(l, st("Settings")), l)
        self.assertIsNone(devui.tap_refusal("Display", st("Settings")))


class StateTests(unittest.TestCase):
    def test_parse_and_button_lookup(self):
        d = {"app": "Settings", "page": "Display", "screen": [2560, 1600], "focused": "", "focused_secret": False, "cut": False,
             "lines": [{"text": "✓ Dark", "x": 2241, "y": 270}], "buttons": [{"label": "✓ Dark", "x": 2229, "y": 263, "w": 78, "h": 30},
                                                                                {"label": "Light", "x": 2314, "y": 263, "w": 64, "h": 30}]}
        s = devui.parse_state(json.dumps(d))
        self.assertEqual((s.app, s.page, s.width, s.height), ("Settings", "Display", 2560, 1600))
        self.assertEqual(s.button("Light").centre, (2346, 278))
        self.assertEqual(s.button("Dark").label, "✓ Dark")  # a part of a label when only one matches
        self.assertIsNone(s.button("Nope"))
        self.assertEqual(builtin_scenarios.checked_choice(s, builtin_scenarios.THEMES), "Dark")

    def test_checked_choice_size_percent(self):
        s = st("Settings", lines=[(1, 1, "✓ Large 125%")])
        self.assertEqual(builtin_scenarios.checked_choice(s, builtin_scenarios.SIZES), "Large")

    def test_overflow_is_sideways_only(self):
        t = devui.Test.__new__(devui.Test)
        t.checks = []
        self.assertTrue(devui.Test.assert_no_overflow(t, st(lines=[(10, 5000, "scrolled")])))
        self.assertFalse(devui.Test.assert_no_overflow(t, st(lines=[(2700, 5, "off right")])))
        self.assertFalse(devui.Test.assert_no_overflow(t, st(cut=True)))


class ScrollbarTests(unittest.TestCase):
    BG, THUMB = (30, 30, 30), (200, 200, 200)

    def strip(self, h=100, w=20, thumb=None, x0=14, x1=18):
        rows = []
        for y in range(h):
            row = [self.BG] * w
            if thumb and thumb[0] <= y < thumb[1]:
                for x in range(x0, x1):
                    row[x] = self.THUMB
            rows.append(row)
        return rows

    def test_a_narrow_uniform_band_is_a_thumb(self):
        self.assertEqual(devui.find_scrollbar(self.strip(thumb=(20, 55))), (14, 20, 55))

    def test_a_plain_strip_and_a_tiny_speck_have_no_scrollbar(self):
        self.assertIsNone(devui.find_scrollbar(self.strip()))
        self.assertIsNone(devui.find_scrollbar(self.strip(thumb=(10, 12))))
        self.assertIsNone(devui.find_scrollbar([]))

    def test_a_wide_block_is_content_not_a_scrollbar(self):
        self.assertIsNone(devui.find_scrollbar(self.strip(w=60, thumb=(10, 60), x0=5, x1=55)))

    def test_the_wheel_is_a_vertical_swipe_after_the_guard(self):
        calls = []
        d = devui.Device.__new__(devui.Device)
        d.pause = 0
        d.try_state = lambda: st("Settings")
        d.run = lambda args, retries=0: calls.append(args)
        d.scroll(500, 800)
        self.assertEqual(calls, [["input", "swipe", "500", "800", "500", "500", "200"]])
        d.scroll(500, 800, down=False)
        self.assertEqual(calls[-1][5], "1100")


TSV = ("level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n"
       "4\t1\t1\t1\t1\t0\t10\t10\t200\t30\t-1\t\n"
       "5\t1\t1\t1\t1\t1\t100\t200\t60\t30\t96.5\tHello\n"
       "5\t1\t1\t1\t1\t2\t170\t200\t80\t30\t91\tWorld,\n"
       "5\t1\t1\t1\t2\t1\t100\t300\t90\t30\t88\tDisplay\n"
       "5\t1\t1\t1\t2\t2\t200\t300\t10\t30\t-1\t \n")


class TsvTests(unittest.TestCase):
    def test_parse(self):
        w = devui.parse_tsv(TSV)
        self.assertEqual([x.text for x in w], ["Hello", "World,", "Display"])
        self.assertAlmostEqual(w[0].conf, 96.5)
        self.assertEqual(devui.words_text(w), "Hello World,\nDisplay")

    def test_find_phrase(self):
        w = devui.parse_tsv(TSV)
        self.assertEqual(devui.find_phrase(w, "hello world"), (100, 200, 250, 230))
        self.assertEqual(devui.find_phrase(w, "Display"), (100, 300, 190, 330))
        self.assertIsNone(devui.find_phrase(w, "World Display"))  # not on one line
        self.assertIsNone(devui.find_phrase(w, "missing"))

    def test_scale(self):
        self.assertEqual(devui.scale_point(100, 50, (1280, 800), (2560, 1600)), (200, 100))
        self.assertEqual(devui.scale_point(7, 7, (0, 0), (10, 10)), (7, 7))


class ImageMathTests(unittest.TestCase):
    def test_luminance_and_ratio(self):
        self.assertAlmostEqual(devui.rel_luminance((255, 255, 255)), 1.0, places=3)
        self.assertAlmostEqual(devui.rel_luminance((0, 0, 0)), 0.0, places=3)
        self.assertAlmostEqual(devui.contrast_ratio(1.0, 0.0), 21.0, places=1)
        self.assertAlmostEqual(devui.contrast_ratio(0.0, 1.0), 21.0, places=1)

    def test_region_contrast(self):
        black_on_white = [(255, 255, 255)] * 800 + [(0, 0, 0)] * 200
        self.assertGreater(devui.region_contrast(black_on_white), 15)
        grey = [(120, 120, 120)] * 800 + [(130, 130, 130)] * 200
        self.assertLess(devui.region_contrast(grey), 1.2)
        self.assertEqual(devui.region_contrast([(9, 9, 9)] * 500), 1.0)
        self.assertEqual(devui.region_contrast([]), 1.0)

    def test_blank(self):
        flat = [(30, 30, 30)] * 1000
        self.assertTrue(devui.is_blank(devui.blank_stats(flat)))
        busy = [(i % 256, (i * 3) % 256, (i * 7) % 256) for i in range(1000)]
        self.assertFalse(devui.is_blank(devui.blank_stats(busy)))
        white = [(255, 255, 255)] * 100
        self.assertTrue(devui.is_blank(devui.blank_stats(white)))


class ScenarioFileTests(unittest.TestCase):
    def test_parse_scn(self):
        head, steps = builtin_scenarios.parse_scn("# c\nname: x\ndevices: real\nrestore: none\nkey s  # open\nwait app=Settings\n")
        self.assertEqual(head, {"name": "x", "devices": "real", "restore": "none"})
        self.assertEqual(steps, ["key s", "wait app=Settings"])

    def test_registry_has_first_scenarios(self):
        for n in ("home-launcher", "settings-pages", "display-theme", "wallpapers", "appearance", "model-picker", "text-size"):
            self.assertIn(n, devui.SCENARIOS)
            self.assertTrue(devui.SCENARIOS[n]["restore"])


class ReportTests(unittest.TestCase):
    def test_report_and_json(self):
        with tempfile.TemporaryDirectory() as d:
            result = {"device": "dev<1>", "kind": "real", "started": "now", "client_version": "v", "notes": ["n"],
                      "scenarios": [{"name": "a", "status": "pass", "reason": "", "restore": "none", "duration": 1.0,
                                     "checks": [{"name": "c <ok>", "ok": True, "detail": "d"}, {"name": "s", "ok": True, "skipped": True, "detail": "skipped: x"}],
                                     "notes": [], "shots": [{"file": "missing.png", "label": "l", "index": 1, "ocr": "text & more"}]},
                                    {"name": "b", "status": "fail", "reason": "bad", "restore": "r", "duration": 2, "checks": [{"name": "x", "ok": False, "detail": ""}], "notes": [], "shots": []},
                                    {"name": "c", "status": "skipped", "reason": "not in this build", "restore": "", "duration": 0, "checks": [], "notes": [], "shots": []}]}
            devui.write_json(result, os.path.join(d, "r.json"))
            devui.write_report(result, os.path.join(d, "r.html"), d)
            h = open(os.path.join(d, "r.html")).read()
            self.assertIn("dev&lt;1&gt;", h)
            self.assertIn("1 fail", h)
            self.assertIn("1 pass", h)
            self.assertIn("1 skipped", h)
            self.assertIn("text &amp; more", h)
            self.assertNotIn("<script", h)
            self.assertEqual(json.load(open(os.path.join(d, "r.json")))["scenarios"][1]["status"], "fail")


class SettingsSidebarTests(unittest.TestCase):
    """A Settings page below the fold of the sidebar is found through the search box (Q1, 2026-11-21); nothing is sent blind and the search is emptied again."""

    class Dev(devui.Device):
        def __init__(self, below):
            super().__init__("fake", run_dir=tempfile.gettempdir())
            self.sent, self.typed, self.below, self.focus = [], "", below, ""

        def _fresh(self):
            return self.state()

        def state(self):
            side = [("Overview", 60, 150, 200, 40)]
            if self.typed and self.typed in self.below:
                side = [(self.typed, 60, 150, 200, 40)]
            return st("Settings", buttons=side, focused=self.focus)

        def run(self, args, retries=None, timeout=None):
            self.sent.append(args[:3])
            if args[:3] == ["input", "key", "/"]:
                self.focus = "Search settings"
            if args[:3] == ["input", "key", "Esc"]:
                self.focus = ""
            if args[:2] == ["input", "key"] and args[2] == "Delete":
                self.typed = ""
            if args[:2] == ["input", "text"]:
                self.typed = args[2]
            return ""

    def test_a_page_below_the_fold_is_searched_tapped_and_the_search_emptied(self):
        d = self.Dev(below=["Accessibility"])
        d.pause = 0
        d.click_sidebar("Accessibility")
        sent = [" ".join(s) for s in d.sent]
        self.assertIn("input text Accessibility", sent)
        self.assertTrue(any(s.startswith("input tap") for s in sent))
        self.assertEqual(sent[-1], "input key Esc")
        self.assertIn("input key Delete", sent[-3:])

    def test_a_page_this_build_does_not_have_is_not_found(self):
        d = self.Dev(below=[])
        d.pause = 0
        self.assertIsNone(d.settings_find("Cloud access"))

    def test_a_visible_page_needs_no_search(self):
        d = self.Dev(below=[])
        d.pause = 0
        self.assertIsNotNone(d.settings_find("Overview"))
        self.assertEqual(d.sent, [])


class RunnerTests(unittest.TestCase):
    """run_scenario with a fake device: restore runs when a check fails and when the scenario raises."""

    class Fake(devui.Device):
        def __init__(self, d, app="Home"):
            super().__init__("fake", run_dir=d)
            self.app = app

        def state(self):
            return st(self.app)

        def go_home(self):
            return st("Home")

        def screenshot(self, label, state=None):
            raise AssertionError("no shots in this test")

    def run_one(self, fn, app="Home", kind="real", devices=("real", "qemu")):
        with tempfile.TemporaryDirectory() as d:
            sc = {"name": "t", "fn": fn, "devices": devices, "needs_signin": True, "start_app": "Home", "restore": "r", "doc": ""}
            return devui.run_scenario(self.Fake(d, app), sc, d)

    def test_restore_runs_after_failed_check_and_exception(self):
        calls = []

        def failing(t):
            t.on_restore(lambda: calls.append("restored"), "x")
            t.check("bad", False, "no")
        r = self.run_one(failing)
        self.assertEqual((r["status"], calls), ("fail", ["restored"]))

        def raising(t):
            t.on_restore(lambda: calls.append("restored2"), "y")
            raise RuntimeError("boom")
        r = self.run_one(raising)
        self.assertEqual(r["status"], "error")
        self.assertIn("restored2", calls)

    def test_restore_failure_fails_the_scenario(self):
        def f(t):
            def bad():
                raise RuntimeError("cannot")
            t.on_restore(bad, "z")
            t.check("ok", True)
        self.assertEqual(self.run_one(f)["status"], "fail")

    def test_skip_when_other_app_in_front_or_wrong_device(self):
        self.assertEqual(self.run_one(lambda t: None, app="Chat")["status"], "skipped")
        self.assertEqual(self.run_one(lambda t: None, app="Sign in")["status"], "skipped")
        self.assertEqual(self.run_one(lambda t: None, devices=("qemu",))["status"], "skipped")

    def test_guard_refusal_is_an_error_not_a_send(self):
        def f(t):
            raise devui.Refused("no")
        self.assertEqual(self.run_one(f)["status"], "error")


@unittest.skipUnless(devui.HAVE_PIL, "needs PIL")
class WheelScenarioTests(unittest.TestCase):
    """The shipped wheel-and-scrollbar scenario run against a fake screen: it scrolls only a page that shows a scrollbar, and checks the page moved."""

    class Screen(devui.Device):
        def __init__(self, d, thumb=True, moves=True, buttons=("Backup",)):
            super().__init__("fake", run_dir=d)
            self.buttons = buttons
            self.app, self.page, self.thumb, self.moves = "Home", "", thumb, moves
            self.scrolled, self.n, self.offset = [], 0, 0

        def state(self):
            return st(self.app, self.page, buttons=[(b, 40, 300, 160, 30) for b in self.buttons] if self.app == "Settings" else [])

        def key(self, k):
            self.app = "Settings"

        def wait_for(self, app=None, page=None, text=None, timeout=15):
            return self.state()

        def click_sidebar(self, label):
            self.page = label

        def scroll(self, x, y, down=True, distance=300):
            self.scrolled.append((x, y, down, distance))
            self.offset += 4 if self.moves else 0

        def go_home(self):
            self.app = "Home"
            return self.state()

        def screenshot(self, label, state=None):
            from PIL import Image
            self.n += 1
            img = Image.new("RGB", (200, 100), (30, 30, 30))
            for y in range(100):  # lines of text that move with the page
                if (y + self.offset) % 10 < 3:
                    for x in range(10, 120):
                        img.putpixel((x, y), (220, 220, 220))
            if self.thumb:
                for y in range(20, 60):
                    for x in range(180, 186):
                        img.putpixel((x, y), (200, 200, 200))
            path = os.path.join(self.run_dir, "%02d-%s.png" % (self.n, label))
            img.save(path)
            return devui.Shot(path, label, self.n, state)

    def run_it(self, name="settings-wheel-scrollbar", **kw):
        with tempfile.TemporaryDirectory() as d:
            dev = self.Screen(d, **kw)
            return dev, devui.run_scenario(dev, devui.SCENARIOS[name], d)

    def test_the_network_page_scenario_scrolls_down_and_back_up_and_is_skipped_when_it_fits(self):
        self.assertIn("settings-wheel-network", devui.SCENARIOS)
        dev, r = self.run_it("settings-wheel-network", buttons=("Network",))
        self.assertEqual(r["status"], "pass", r)
        self.assertEqual([c[2] for c in dev.scrolled], [True, False])
        dev, r = self.run_it("settings-wheel-network", buttons=("Network",), thumb=False)
        self.assertEqual((r["status"], dev.scrolled), ("skipped", []))

    def test_a_page_with_a_scrollbar_is_scrolled_once_in_the_middle_and_must_move(self):
        dev, r = self.run_it()
        self.assertEqual(r["status"], "pass", r)
        self.assertEqual(dev.scrolled, [(100, 50, True, 300)])

    def test_a_page_that_fits_is_skipped_and_the_wheel_is_never_used(self):
        dev, r = self.run_it(thumb=False)
        self.assertEqual((r["status"], dev.scrolled), ("skipped", []))

    def test_a_wheel_that_moves_nothing_fails_the_scenario(self):
        dev, r = self.run_it(moves=False)
        self.assertEqual(r["status"], "fail", r)

    def test_bad_wheel_steps_are_refused_in_words(self):
        head, steps = builtin_scenarios.parse_scn("scroll sideways\n")
        self.assertEqual(steps, ["scroll sideways"])


class PairScriptTests(unittest.TestCase):
    """dist/qemu/dev-pair-test.py can be imported by a runner: nothing runs at import and `pair_guest` is the one function to call."""

    def load(self):
        import importlib.util
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "qemu", "dev-pair-test.py")
        spec = importlib.util.spec_from_file_location("dev_pair_test", path)
        m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(m)
        return m

    def test_import_runs_nothing_and_offers_the_functions(self):
        m = self.load()
        for name in ("pair_guest", "run_vm", "configure", "main"):
            self.assertTrue(callable(getattr(m, name)), name)
        self.assertEqual((m.OUT, m.usb, m.out_dir, m.fails), (None, None, None, []))

    def test_configure_says_what_is_missing_instead_of_booting(self):
        m = self.load()
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(SystemExit) as e:
                m.configure([d], {"PATH": ""})
            self.assertIn("needs", str(e.exception))
            self.assertEqual(m.fails, [])


class SharedPairingTests(unittest.TestCase):
    """qemu_guest.py reuses the pairing flow of dist/qemu/dev-pair-test.py (the `Vm` class) instead of mirroring it."""

    def fake_vm(self):
        import qemu_guest
        m = qemu_guest.load_pair_script()
        vm = m.Vm.__new__(m.Vm)
        vm.tag, vm.port, vm.fails, vm.keys, vm.texts = "t", 17979, [], [], []
        vm.cmd = lambda c, wait=0.15: vm.keys.append(c)
        vm.typ = lambda text: vm.keys.append("typ:" + text)
        return m, vm

    def test_the_guest_class_builds_on_the_shared_vm_and_keeps_no_copy_of_the_recipe(self):
        import qemu_guest
        src = open(os.path.join(os.path.dirname(os.path.abspath(qemu_guest.__file__)), "qemu_guest.py")).read()
        self.assertNotIn("\"-accel\"", src)
        self.assertNotIn("screendump", src)
        m = qemu_guest.load_pair_script()
        for name in ("boot", "close", "snap", "shot", "make_account", "read_pair_code", "pair_from_host"):
            self.assertTrue(callable(getattr(m.Vm, name)), name)
        self.assertEqual(qemu_guest.SCRATCH_PASSWORD, m.SCRATCH_PASSWORD)

    def test_the_pairing_code_is_read_from_the_screen_and_the_firewall_allow_is_pressed(self):
        m, vm = self.fake_vm()
        screens = iter(["Developer mode is on", "Pair: AB3D5 - 9XY2Z", "the firewall allows the local network for ten minutes"])
        vm.page = lambda q: "page"
        vm.shot = lambda name: next(screens)
        vm.wait_for = lambda what, secs, name: vm.shot(name)
        vm.check = lambda *a: None
        self.assertEqual(vm.read_pair_code(), "AB3D5-9XY2Z")
        self.assertEqual(vm.keys[-1], "sendkey f")

    def test_no_code_on_the_screen_is_a_noted_failure_not_a_crash(self):
        m, vm = self.fake_vm()
        vm.page = lambda q: ""
        vm.shot = lambda name: "nothing here"
        vm.wait_for = lambda what, secs, name: "nothing here"
        vm.check = lambda *a: None
        self.assertIsNone(vm.read_pair_code())
        self.assertTrue(any("no pairing code" in f for f in vm.fails))
        del m.fails[:]

    def test_pair_from_host_names_the_port_and_the_device(self):
        m, vm = self.fake_vm()
        calls = []
        class R:  # a finished process
            returncode = 0
        orig = m.subprocess.run
        m.subprocess.run = lambda argv, **kw: (calls.append((argv, kw["env"]["HOME"])), R())[1]
        try:
            with tempfile.TemporaryDirectory() as d:
                home = os.path.join(d, "h")
                vm.pair_from_host("/x/iyakki", "AAAAA-BBBBB", home, name="qemu-guest")
        finally:
            m.subprocess.run = orig
        argv, home_used = calls[0]
        self.assertEqual(argv, ["/x/iyakki", "dev", "pair", "127.0.0.1:17979", "--code", "AAAAA-BBBBB", "--as", "qemu-guest", "--home", home])
        self.assertEqual(home_used, home)


if __name__ == "__main__":
    unittest.main()
