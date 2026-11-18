#!/usr/bin/env python3
"""Checks of the layer decision of fast-loop.py (no cargo, no device): python3 dist/scripts/test-fast-loop.py"""
import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("fast_loop", os.path.join(HERE, "fast-loop.py"))
fl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fl)


def plan(changed, layers, root=False):
    return {"changed": changed, "layers": layers, "root": root}


class Decide(unittest.TestCase):
    def test_an_app_change_is_live_ui(self):
        v, _ = fl.decide(["crates/iyakki-apps/src/apps/radio.rs"], plan(["iyakki-apps"], {"iyakki-apps": "apps"}))
        self.assertEqual(v, "live-ui")

    def test_a_core_or_agent_change_is_live_core(self):
        v, _ = fl.decide(["crates/iyakki-core/src/main.rs"], plan(["iyakki-core"], {"iyakki-core": "core"}))
        self.assertEqual(v, "live-core")
        v, _ = fl.decide(["a", "b"], plan(["iyakki-apps", "iyakki-agent"], {"iyakki-apps": "apps", "iyakki-agent": "core"}))
        self.assertEqual(v, "live-core")

    def test_kernel_boot_and_image_recipe_changes_need_a_reboot_even_with_code(self):
        for f in ["dist/buildroot/linux.fragment", "iyakki-boot/src/main.rs", "crates/iyakki-stage1/src/lib.rs", "dist/buildroot/board/pc/x"]:
            v, _ = fl.decide([f, "crates/iyakki-apps/src/lib.rs"], plan(["iyakki-apps"], {"iyakki-apps": "apps"}))
            self.assertEqual(v, "reboot", f)

    def test_docs_only_is_nothing_and_a_root_file_is_live_core(self):
        self.assertEqual(fl.decide(["docs/x.md"], plan([], {}))[0], "nothing")
        self.assertEqual(fl.decide(["Cargo.lock"], plan([], {}, root=True))[0], "live-core")


    def test_an_agent_change_is_live_core(self):
        v, _ = fl.decide(["crates/iyakki-agent/src/x.rs"], plan(["iyakki-agent"], {"iyakki-agent": "agent"}))
        self.assertEqual(v, "live-core")


class Apply(unittest.TestCase):
    def test_the_apply_step_asks_the_running_program_to_exec_the_pushed_one(self):
        c = fl.apply_argv("/b/iyakki", {"--device": "lab", "--home": None})
        self.assertEqual(c[:4], ["/b/iyakki", "dev", "--device", "lab"])
        self.assertEqual(c[c.index("--") + 1:], ["/usr/bin/iyakki", "live-restart", fl.LIVE_PROGRAM])
        self.assertIn("shell", c)

    def test_a_fetch_step_runs_before_the_tests_and_gets_the_binary_path(self):
        src = open(os.path.join(HERE, "fast-loop.py")).read()
        self.assertLess(src.index('st.run("fetch"'), src.index('st.run("test"'))
        self.assertIn("FAST_BIN", src)


if __name__ == "__main__":
    unittest.main()
