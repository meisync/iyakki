"""The autotest verbs added to the dist/devtest `.scn` format (no new format; the safety guard of devui still checks every input).

  expect-golden STEP [mask=x,y,w,h ...] [budget=PERCENT]   the shot labelled STEP against dist/autotest/golden/<ROW>/STEP.png (a missing golden is NEW)
  expect-audit REGEX                                       the audit tail of the device (`dev logs --system --source audit`) matches
  expect-serial REGEX                                      the guest's serial log matches (QEMU only)
  expect-sh COMMAND ==> REGEX                              a command in the guest through `dev shell` (answered by the dev machine switch) prints a match
  expect-info REGEX                                        `dev info` (the compatibility report and the device lines) matches
  expect-config KEY VALUE                                  `dev config get KEY` equals VALUE
  expect-http URL STATUS [REGEX]                           a loopback URL ({http} is the forwarded web port) answers STATUS (and the body matches)
  covers N                                                 documentation only: marks that the checks above this line belong to clause N (no effect on the result)

Sound verbs (listen.py judges the guest's recording of this scenario's window; the QEMU session records with -audiodev wav, see AUTOTEST.md):
  expect-tone HZ [tol] [left|right]                        a tone at HZ within tol Hz (default 10) is there, at -40 dBFS or louder, for 0.2 s or more
  expect-level LOW HIGH                                    the RMS of the window is between LOW and HIGH dBFS
  expect-silence                                           no window of the recording is louder than -60 dBFS
  expect-sound-within SECONDS                              the first sound starts no later than SECONDS after the scenario began
  expect-speech MIN MAX                                    the voiced span (energy envelope, hangover) lasts MIN to MAX seconds; it does NOT recognise words
A sound verb with no recording is refused (the check fails with the reason), never passed.

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
import listen  # noqa: E402

SOUND_VERBS = ("expect-tone", "expect-level", "expect-silence", "expect-sound-within", "expect-speech")


class Active:
    row = ""
    golden_root = os.path.join(HERE, "golden")
    update_golden = False
    sh = None            # callable(command) -> (exit code, output), or None
    serial = None        # callable() -> text, or None
    http_base = None     # e.g. "http://127.0.0.1:8123"
    new_goldens = []     # [(row, step, path)] the runner turns into NEW-GOLDEN
    golden_diffs = []    # [(row, step, diff png path)]
    wav_path = None      # the QEMU guest's recording (-audiodev wav), or None: the sound verbs are then refused
    wav_start = 0       # frames already in that file when this scenario began: the window the sound verbs judge

    @classmethod
    def reset(cls, row="", **kw):
        cls.row, cls.sh, cls.serial, cls.http_base = row, None, None, None
        cls.update_golden, cls.golden_root = False, os.path.join(HERE, "golden")
        cls.new_goldens, cls.golden_diffs = [], []
        cls.wav_path, cls.wav_start = None, 0
        for k, v in kw.items():
            setattr(cls, k, v)
        if cls.wav_path and os.path.isfile(cls.wav_path):
            cls.wav_start = listen.frame_count(cls.wav_path)


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


def uses_sound(steps):
    """True when a scenario's steps (text lines) use a sound verb: the QEMU session must then record its audio."""
    return any(line.split(" ", 1)[0] in SOUND_VERBS for line in steps)


def _recording():
    """The guest recording of this scenario's window. Refused (a failed check with the reason) when there is none."""
    if not Active.wav_path:
        raise devui.CheckFailed("refused: a sound check needs the guest's recording, and this run has none (the QEMU profile records with -audiodev wav; targets: qemu)")
    if not os.path.isfile(Active.wav_path):
        raise devui.CheckFailed("refused: no recording at %s (the guest did not write its audio)" % Active.wav_path)
    try:
        return listen.load(Active.wav_path, start_frame=Active.wav_start)
    except (listen.WavError, OSError) as ex:
        raise devui.CheckFailed("refused: the recording at %s cannot be read: %s" % (Active.wav_path, ex))


def _number(word, usage):
    try:
        return float(word)
    except ValueError:
        raise devui.CheckFailed("%s: %r is not a number" % (usage, word))


def v_expect_tone(t, arg):
    words = arg.split()
    usage = "expect-tone HZ [tol] [left|right]"
    if not words:
        raise devui.CheckFailed(usage)
    hz, tol, side = _number(words[0], usage), 10.0, None
    for w in words[1:]:
        if w in ("left", "right"):
            side = w
        else:
            tol = _number(w, usage)
    wav = _recording()
    try:
        r = listen.tone_present(wav, hz, tol_hz=tol, channel=side)
    except ValueError as ex:
        raise devui.CheckFailed("refused: %s" % ex)
    where = " on the %s channel" % side if side else ""
    detail = ("%.2f s at %.0f Hz, %.1f dBFS" % (r["seconds"], r["hz"], r["level_dbfs"])) if r["present"] else "no such tone in %.2f s of the recording" % (r["seconds"])
    t.check("tone %g Hz (+-%g Hz)%s" % (hz, tol, where), r["present"], detail)


def v_expect_level(t, arg):
    words = arg.split()
    if len(words) != 2:
        raise devui.CheckFailed("expect-level LOW HIGH (dBFS)")
    low, high = _number(words[0], "expect-level"), _number(words[1], "expect-level")
    lv = listen.level_dbfs(_recording())
    got = lv["rms_dbfs"]
    t.check("level between %g and %g dBFS" % (low, high), low <= got <= high, "RMS %.1f dBFS, peak %.1f dBFS" % (got, lv["peak_dbfs"]))


def v_expect_silence(t, arg):
    ok = listen.silence(_recording())
    t.check("silence (no window above -60 dBFS)", ok, "the recording is silent" if ok else "sound was heard in the recording")


def v_expect_sound_within(t, arg):
    if not arg.strip():
        raise devui.CheckFailed("expect-sound-within SECONDS")
    limit = _number(arg.split()[0], "expect-sound-within")
    wav = _recording()
    at = listen.first_sound_at(wav)
    t.check("sound starts within %g s" % limit, at is not None and at <= limit, "no sound at all" if at is None else "first sound at %.2f s" % at)


def v_expect_speech(t, arg):
    words = arg.split()
    if len(words) != 2:
        raise devui.CheckFailed("expect-speech MIN MAX (seconds of voiced span; it does not recognise words)")
    lo, hi = _number(words[0], "expect-speech"), _number(words[1], "expect-speech")
    r = listen.speech_like(_recording(), lo, hi)
    t.check("voice-like activity %g to %g s" % (lo, hi), r["ok"], "voiced span %.2f s in %d burst(s)" % (r["seconds"], r["bursts"]))


def register():
    builtin_scenarios.EXTRA_VERBS.update({"expect-golden": v_expect_golden, "expect-audit": v_expect_audit, "expect-serial": v_expect_serial,
                                          "expect-sh": v_expect_sh, "expect-info": v_expect_info, "expect-config": v_expect_config, "expect-http": v_expect_http, "covers": v_covers,
                                          "expect-tone": v_expect_tone, "expect-level": v_expect_level, "expect-silence": v_expect_silence,
                                          "expect-sound-within": v_expect_sound_within, "expect-speech": v_expect_speech})


register()
