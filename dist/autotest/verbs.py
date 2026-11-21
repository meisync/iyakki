"""The autotest verbs added to the dist/devtest `.scn` format (no new format; the safety guard of devui still checks every input).

  expect-golden STEP [mask=x,y,w,h ...] [budget=PERCENT]   the shot labelled STEP against dist/autotest/golden/<ROW>/STEP.png (a missing golden is NEW)
  expect-audit REGEX                                       the audit tail of the device (`dev logs --system --source audit`) matches
  expect-serial REGEX                                      the guest's serial log matches (QEMU only)
  expect-sh COMMAND ==> REGEX                              a command in the guest through `dev shell` (answered by the dev machine switch) prints a match
  expect-info REGEX                                        `dev info` (the compatibility report and the device lines) matches
  expect-config KEY VALUE                                  `dev config get KEY` equals VALUE
  expect-http URL STATUS [REGEX]                           a loopback URL ({http} is the forwarded web port) answers STATUS (and the body matches)
  covers N                                                 documentation only: marks that the checks above this line belong to clause N (no effect on the result)

`ACTIVE` is the runner's hand-over: current row, golden folder, the update switch and the guest helpers. Pure when `ACTIVE` is faked.
"""
import os
import re
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "devtest"))
sys.path.insert(0, HERE)
import builtin_scenarios  # noqa: E402
import devui  # noqa: E402
import golden  # noqa: E402


class Active:
    row = ""
    golden_root = os.path.join(HERE, "golden")
    update_golden = False
    sh = None            # callable(command) -> (exit code, output), or None
    serial = None        # callable() -> text, or None
    http_base = None     # e.g. "http://127.0.0.1:8123"
    new_goldens = []     # [(row, step, path)] the runner turns into NEW-GOLDEN
    golden_diffs = []    # [(row, step, diff png path)]

    @classmethod
    def reset(cls, row="", **kw):
        cls.row, cls.sh, cls.serial, cls.http_base = row, None, None, None
        cls.update_golden, cls.golden_root = False, os.path.join(HERE, "golden")
        cls.new_goldens, cls.golden_diffs = [], []
        for k, v in kw.items():
            setattr(cls, k, v)


def _find_shot(t, step):
    for s in reversed(t.shots):
        if s.label == step:
            return s
    return t.shot(step, t.dev.state())


def v_expect_golden(t, arg):
    words = arg.split()
    if not words:
        raise devui.CheckFailed("expect-golden needs a step name")
    step, rest = words[0], words[1:]
    masks = golden.parse_masks([w for w in rest if w.startswith("mask=")])
    budget = golden.DEFAULT_BUDGET
    for w in rest:
        if w.startswith("budget="):
            budget = float(w.split("=", 1)[1])
    if not golden.HAVE_PIL:
        t.skipped_check("golden %s" % step, "no PIL")
        return
    shot = _find_shot(t, step)
    gp = golden.golden_path(Active.golden_root, Active.row, step)
    if not os.path.isfile(gp):
        if Active.update_golden:
            os.makedirs(os.path.dirname(gp), exist_ok=True)
            with open(shot.path, "rb") as src, open(gp, "wb") as dst:
                dst.write(src.read())
        Active.new_goldens.append((Active.row, step, shot.path))
        t.note("NEW golden %s/%s: a candidate for review, not a pass" % (Active.row, step))
        return
    if Active.update_golden:
        with open(shot.path, "rb") as src, open(gp, "wb") as dst:
            dst.write(src.read())
        t.note("golden %s/%s rewritten (--update-golden); review it before committing" % (Active.row, step))
        return
    r = golden.compare(gp, shot.path, masks, budget)
    if not r["ok"] and r["size_ok"]:
        dp = os.path.splitext(shot.path)[0] + "-golden-diff.png"
        golden.write_diff_picture(gp, shot.path, r["_hot"], dp)
        Active.golden_diffs.append((Active.row, step, dp))
    t.check("golden %s within %.2f%% (%d pixels counted)" % (step, budget, r["counted"]), r["ok"],
            "%.3f%% changed" % r["changed_percent"] if r["size_ok"] else "the picture size differs from the golden")


def v_expect_audit(t, arg):
    text = t.dev.run(["logs", "--system", "--source", "audit", "-n", "400"])
    m = re.search(arg, text)
    t.check("audit tail matches %r" % arg, m is not None, ("found %r" % m.group(0)[:80]) if m else "not in the last 400 audit lines")


def v_expect_serial(t, arg):
    if Active.serial is None:
        raise devui.Skip("no serial log on this target")
    m = re.search(arg, Active.serial())
    t.check("serial log matches %r" % arg, m is not None, ("found %r" % m.group(0)[:80]) if m else "not in the serial log")


def v_expect_sh(t, arg):
    if Active.sh is None:
        raise devui.Skip("no guest shell on this target")
    cmd, sep, rx = arg.partition("==>")
    if not sep:
        raise devui.CheckFailed("expect-sh is `COMMAND ==> REGEX`")
    code, out = Active.sh(cmd.strip())
    m = re.search(rx.strip(), out)
    t.check("`%s` prints %r" % (cmd.strip()[:60], rx.strip()), code == 0 and m is not None, "exit %s, output: %s" % (code, out[-120:].replace("\n", " | ")))


def v_expect_info(t, arg):
    text = t.dev.info()
    m = re.search(arg, text)
    t.check("dev info matches %r" % arg, m is not None, ("found %r" % m.group(0)[:80]) if m else "not in `dev info`")


def v_expect_config(t, arg):
    key, _, want = arg.partition(" ")
    got = t.dev.config_get(key)
    val = got.split("=", 1)[1].strip() if "=" in got else got.strip()
    t.check("config %s is %s" % (key, want.strip()), val == want.strip(), "read %r" % val)


def v_expect_http(t, arg):
    words = arg.split(None, 2)
    if len(words) < 2:
        raise devui.CheckFailed("expect-http is `URL STATUS [REGEX]`")
    url = words[0]
    if "{http}" in url:
        if not Active.http_base:
            raise devui.Skip("no forwarded web port on this target")
        url = url.replace("{http}", Active.http_base)
    if not re.match(r"^http://(127\.0\.0\.1|localhost)[:/]", url):
        raise devui.CheckFailed("expect-http only talks to loopback, not %s" % url)
    try:
        with urllib.request.urlopen(url, timeout=10) as r:
            status, body = r.status, r.read(65536).decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        status, body = e.code, ""
    ok = str(status) == words[1] and (len(words) < 3 or re.search(words[2], body) is not None)
    t.check("%s answers %s" % (url, words[1]), ok, "status %s" % status)


def v_covers(t, arg):
    t.note("covers clause %s" % arg)


def register():
    builtin_scenarios.EXTRA_VERBS.update({"expect-golden": v_expect_golden, "expect-audit": v_expect_audit, "expect-serial": v_expect_serial,
                                          "expect-sh": v_expect_sh, "expect-info": v_expect_info, "expect-config": v_expect_config, "expect-http": v_expect_http, "covers": v_covers})


register()
