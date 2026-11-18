"""The checks of dist/qemu/session-checks.py. Each is `check_<name>(s)` where `s` is a session_lib.Session; it returns the evidence text and
raises Fail (or AssertionError) when the guest did not show what the row promises. A check that cannot run raises devui.Skip."""
import json, re, time


class Fail(AssertionError):
    pass


def need(cond, msg):
    if not cond:
        raise Fail(msg)


def shell_ok(s, cmd, timeout=90):
    rc, out = s.sh(cmd, timeout)
    return rc, out


def check_dev_doctor(s):
    rc, out = s.dev("doctor")
    need(rc == 0, "`iyakki dev doctor` exited %s: %s" % (rc, out[-200:]))
    need(re.search(r"\[(Problem|Note)\]", out), "no finding line in the report: %r" % out[:200])
    need("Not looked at from the command line" not in out, "the dev report carries the offline caveat (it should hold the running system's state)")
    rc2, out2 = s.dev("doctor", "--detail")
    need(rc2 == 0 and len(out2) >= len(out), "--detail printed less than the plain report")
    rc3, off = s.sh("iyakki doctor")
    need("Not looked at from the command line" in off, "the offline `iyakki doctor` did not say it cannot see the running system")
    return "dev doctor:\n%s\n--detail lines: %d vs %d\noffline `iyakki doctor` (in the guest) ends with the caveat: yes" % (out, len(out2.splitlines()), len(out.splitlines()))


def check_dev_info_compat(s):
    rc, out = s.dev("info")
    need(rc == 0, "dev info exited %s" % rc)
    for pat, what in [(r"compat\s+core \S+ · contract", "core and contract versions"), (r"client protocol", "client protocol"), (r"compat\s+peer computer .*developer mode", "the paired computer as a peer"),
                      (r"compat\s+deprecated", "the deprecated names list")]:
        need(re.search(pat, out), "dev info lacks %s" % what)
    return "\n".join(l for l in out.splitlines() if "compat" in l)[:1800]


def check_coldplug_extra_pass(s):
    ser = s.serial()
    m = re.search(r"coldplug: (\d+) more device aliases appeared after drivers loaded \((\d+) extra passes?\)[^\n]*", ser)
    need(m, "the boot log has no 'coldplug: N more device aliases appeared after drivers loaded' line")
    need(int(m.group(2)) <= 3, "more than 3 extra passes: %s" % m.group(0))
    return m.group(0)


def check_components_record(s):
    rc, out = s.sh("cat /var/lib/iyakki/components-installed.json; echo ===; uname -r; iyakki --version | head -1; . /etc/iyakki-release; echo REL=$VERSION; sha256sum /usr/bin/iyakki")
    need(rc == 0, "reading the record failed: %s" % out[-200:])
    j, rest = out.split("===", 1)
    j = j[j.index("{"):]
    rec = json.loads(j)
    lines = [l.strip() for l in rest.strip().splitlines() if l.strip()]
    kernel, ver, rel, sha = lines[0], lines[1], [l for l in lines if l.startswith("REL=")][0][4:], lines[-1].split()[0]
    need(rec["versions"]["kernel"] == kernel, "record kernel %s != running %s" % (rec["versions"]["kernel"], kernel))
    need(rec["versions"]["core"] in ver, "record core %s is not the program version %s" % (rec["versions"]["core"], ver))
    need(rec.get("release") == rel, "record release %s != release file %s" % (rec.get("release"), rel))
    need(rec["shas"]["core"] == sha, "record sha of the program differs from sha256sum of /usr/bin/iyakki")
    need(set(rec["how"].values()) == {"observed"}, "a version was not corrected from facts: %s" % rec["how"])
    return "record: %s\nrunning: kernel %s, %s, release %s, sha %s" % (json.dumps(rec["versions"]), kernel, ver, rel, sha[:16])


STORE = r"""
cd /tmp/iyakki-dev || exit 1
rm -rf st sp; mkdir st sp; cd sp
printf '%s' 'tok-0123456789-0123456789-abcdef' > tok
iyakki pkg keygen cat >/dev/null
printf -- '---\nname: hello\ndescription: say hello\n---\nSay hello.\n' > hello.md
iyakki pkg keygen pub >/dev/null
iyakki pkg build hello.md --kind skill --version 1.0.0 --license MIT -o hello.iyp >/dev/null
iyakki pkg sign hello.iyp --key pub.key -o hello.signed.iyp >/dev/null
setsid iyakki store-server --data /tmp/iyakki-dev/st --listen 127.0.0.1:18080 --publish-token-file /tmp/iyakki-dev/sp/tok --catalog-key-file /tmp/iyakki-dev/sp/cat.key </dev/null >st.log 2>&1 &
echo $! > pid
sleep 3
echo "HEALTH=$(wget -q -O - http://127.0.0.1:18080/healthz 2>&1 | head -c 50)"
echo "CATALOG0=$(wget -q -O - http://127.0.0.1:18080/catalog.json 2>&1 | tr -d '\n ')"
echo "PUBLISH=$(TOK=$(cat tok) iyakki pkg publish --server http://127.0.0.1:18080 --token-env TOK hello.signed.iyp 2>&1 | head -c 300)"
echo "CATALOG1=$(wget -q -O - http://127.0.0.1:18080/catalog.json 2>&1 | tr -d '\n ')"
echo "BADPUB=$(TOK=wrong-token-wrong-token-wrong iyakki pkg publish --server http://127.0.0.1:18080 --token-env TOK hello.signed.iyp 2>&1 | head -c 200)"
kill $(cat pid) 2>/dev/null
echo "STLOG=$(head -c 300 st.log | tr '\n' ' ')"
"""


def kv(out):
    d = {}
    for l in out.splitlines():
        m = re.match(r"([A-Z0-9]+)=(.*)$", l)
        if m:
            d[m.group(1)] = m.group(2)
    return d


def check_private_store_roundtrip(s):
    rc, out = s.sh(STORE, 120)
    d = kv(out)
    need(d.get("HEALTH") == "ok", "healthz said %r (%s)" % (d.get("HEALTH"), out[-300:]))
    c0 = json.loads(d["CATALOG0"])
    need(c0["entries"] == [] and len(c0.get("sig", "")) == 128, "the empty catalog is not signed: %s" % d["CATALOG0"][:200])
    need("listed as Community" in d.get("PUBLISH", ""), "publish: %s" % d.get("PUBLISH"))
    c1 = json.loads(d["CATALOG1"])
    need([e["id"] for e in c1["entries"]] == ["hello"] and c1["serial"] > c0["serial"], "the catalog does not list the package: %s" % d["CATALOG1"][:300])
    sha = re.search(r"sha256 ([0-9a-f]{64})", d["PUBLISH"])
    need(sha and c1["entries"][0]["sha256"] == sha.group(1), "the catalog's hash differs from the published one")
    need("published" not in d.get("BADPUB", "published"), "a wrong token was accepted or not answered: %s" % d.get("BADPUB"))
    return "healthz=ok; catalog0 serial %s entries 0 signed; publish: %s; catalog1 serial %s entries %s; wrong token: %s" % (c0["serial"], d["PUBLISH"], c1["serial"], [e["id"] for e in c1["entries"]], d.get("BADPUB"))


TRUST = r"""
cd /tmp/iyakki-dev || exit 1
rm -rf tp; mkdir tp; cd tp
iyakki pkg keygen pub >/dev/null; iyakki pkg keygen root >/dev/null
printf -- '---\nname: hello\ndescription: say hello\n---\nSay hello.\n' > hello.md
iyakki pkg build hello.md --kind skill --version 1.0.0 --license MIT -o plain.iyp >/dev/null
iyakki pkg sign plain.iyp --key pub.key -o signed.iyp >/dev/null
cp pub.pub keys.txt; echo "r1 $(cat root.pub)" > roots.txt
iyakki pkg cert-issue --root root.key --publisher acme --name Acme --key $(cat pub.pub) --scopes skill > cert.json
iyakki pkg sign plain.iyp --key pub.key --cert cert.json -o cert.iyp >/dev/null
iyakki pkg cert-issue --root root.key --publisher acme --name Acme --key $(cat pub.pub) --scopes tool > cert-tool.json
iyakki pkg sign plain.iyp --key pub.key --cert cert-tool.json -o cert-tool.iyp >/dev/null
iyakki pkg revoke-sign --root root.key --serial 1 --bad-key $(cat pub.pub) -o rev.json >/dev/null
echo "UNSIGNED=$(iyakki pkg verify plain.iyp 2>&1 | grep '^trust:')"
echo "COMMUNITY=$(iyakki pkg verify signed.iyp 2>&1 | grep '^trust:')"
echo "TRUSTED=$(iyakki pkg verify signed.iyp --trusted keys.txt 2>&1 | grep '^trust:')"
echo "VERIFIED=$(iyakki pkg verify cert.iyp --roots roots.txt 2>&1 | grep '^trust:')"
echo "SCOPE=$(iyakki pkg verify cert-tool.iyp --roots roots.txt 2>&1 | grep -i '^trust:\|scope' | tr '\n' ' ')"
echo "REVOKED=$(iyakki pkg verify cert.iyp --roots roots.txt --revocations rev.json 2>&1 | head -4 | tr '\n' ' ')"
echo "FORGED=$(iyakki pkg verify cert.iyp --roots /dev/null 2>&1 | grep '^trust:')"
"""


def check_pkg_trust_levels(s):
    rc, out = s.sh(TRUST, 120)
    d = kv(out)
    need(d.get("UNSIGNED", "").startswith("trust: Unsigned"), "unsigned: %r" % d.get("UNSIGNED"))
    need(d.get("COMMUNITY", "").startswith("trust: Community"), "community: %r" % d.get("COMMUNITY"))
    need(d.get("TRUSTED", "").startswith("trust: Trusted"), "trusted: %r" % d.get("TRUSTED"))
    need(d.get("VERIFIED", "").startswith("trust: Verified"), "verified: %r (%s)" % (d.get("VERIFIED"), out[-400:]))
    need(d.get("SCOPE", "").startswith("trust: Community"), "an out-of-scope certificate did not fall back to Community: %r" % d.get("SCOPE"))
    need("Revoked" in d.get("REVOKED", ""), "revoked: %r" % d.get("REVOKED"))
    need(d.get("FORGED", "").startswith("trust: Community"), "a certificate from an unpinned root: %r" % d.get("FORGED"))
    return "\n".join("%s -> %s" % (k, d[k]) for k in ("UNSIGNED", "COMMUNITY", "TRUSTED", "VERIFIED", "SCOPE", "REVOKED", "FORGED"))


def check_device_name(s):
    rc, out = s.sh("cat /var/lib/iyakki/device-id; echo; echo ===; cat /var/lib/iyakki/devname.json; echo; echo ===; cat /var/lib/iyakki/firewall.json")
    need(rc == 0, out[-200:])
    ident, dn, fw = [p.strip() for p in out.split("===")][:3] if out.count("===") >= 2 else (None, None, None)
    ident = ident.splitlines()[-1].strip()
    need(re.fullmatch(r"iyakki-[0-9a-f]{6}", ident), "the device id %r is not iyakki-<3 random bytes>" % ident)
    dnj = json.loads(dn[dn.index("{"):])
    need(dnj["on"] and dnj["host"] == ident + ".local", "the announced host %r is not the device id" % dnj.get("host"))
    need(any(x.startswith("_iyakki-dev._tcp") for x in dnj["services"]), "developer mode is on but the dev service is not announced: %s" % dnj["services"])
    cfg = json.loads(fw[fw.index("{"):])["config"]
    mdns = [r for r in cfg["rules"] if r["comment"] == "device name (mDNS)"]
    need({r["value"] for r in mdns} == {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16"}, "the mDNS rule is not limited to the four private ranges: %s" % [r["value"] for r in mdns])
    need(all(r["proto"] == "udp" and r["ports"] == "5353" for r in mdns), "the mDNS rule is not UDP 5353 only")
    rc, diag = s.dev("diag")
    need(re.search(r"device name \(mDNS\): %s\.local" % re.escape(ident), diag), "dev diag does not print the name")
    rc, st = s.dev("status")
    need(re.search(r"Device name \(mDNS\)\s+ready", st), "the startup list has no ready 'Device name (mDNS)' row")
    return "device-id %s; announced %s services %s; firewall rules %s (udp 5353); dev diag prints the name; startup row ready" % (ident, dnj["host"], dnj["services"], sorted(r["value"] for r in mdns))


def check_dev_approval_banner(s):
    b = s.banner
    need(b, "no shell request banner was seen in `dev state` while the request waited")
    need(re.search(r"^app: Home", b, re.M), "the banner was read while another app was in front")
    need("DEV · SHELL REQUEST from" in b and re.search(r"asks to run commands on this device as", b), "the banner text is not the shell request's")
    need(s.shell_ok, "the request was not answered on the guest's own screen (shell did not run)")
    return "\n".join(l for l in b.splitlines() if "DEV" in l or "asks to run" in l or l.startswith("app:")) + "\nanswered with the 8 hour key on Settings, Developer mode: " + s.approve_log.strip().splitlines()[1]


def check_a11y_labels(s):
    rc, out = s.dev("state", "--json")
    need(rc == 0, "dev state --json failed")
    j = json.loads(out[out.index("{"):])
    btn = j["buttons"]
    need(len(btn) >= 3, "only %d clickable regions" % len(btn))
    unnamed = [b for b in btn if not str(b.get("label", "")).strip()]
    need(not unnamed, "%d clickable regions without a name: %s" % (len(unnamed), unnamed[:2]))
    need(all("secret" in b for b in btn), "a region does not say whether it is secret")
    return "app %s: %d clickable regions, all named, e.g. %s" % (j.get("app"), len(btn), [b["label"][:30] for b in btn[:4]])


def check_clock_floor_lifted(s):
    ser = s.serial()
    m = re.search(r"clock floor \(early\): the clock was behind and is now (\d+) \(unix seconds\)", ser)
    need(m, "no 'clock was behind and is now' line in the boot log (started with the virtual clock at 2024-01-01): %s" % [l for l in ser.splitlines() if "clock floor" in l][:2])
    floor = int(m.group(1))
    import calendar
    need(floor >= calendar.timegm((2026, 11, 1, 0, 0, 0)), "the floor %d is not a date of this image's era" % floor)
    stamp = [l for l in ser.splitlines() if re.match(r"\[\s*\d+\.\d+\] iyakki: \[iyakki INFO \] clock", l)][:3]
    return "QEMU started the virtual hardware clock at 2024-01-01 (unix 1704067200); the boot log says '%s' (unix %d = %s); later clock lines: %s" % (m.group(0), floor, time.strftime("%Y-%m-%d", time.gmtime(floor)), stamp)

