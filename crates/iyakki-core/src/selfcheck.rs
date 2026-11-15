//! The result lines of the guest self-tests that decide for themselves (`iyakki.selftest=sandboxnet` and `=bootlist`, see `dist/qemu/README.md`).
//!
//! The guest prints one `IYAKKI-SELFTEST <mode> check <name> PASS|FAIL <detail>` line per check and a last
//! `IYAKKI-SELFTEST <mode> RESULT PASS|FAIL <passed>/<total>` line. The script that boots the guest reads them with `dist/qemu/selftest-result.py`
//! (same format, tested against this module). A log with no RESULT line, a RESULT that disagrees with its checks or a failed check is a failure.

/// One check: a name without spaces, whether it held, and a short detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

pub fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check { name: name.replace(' ', "-"), ok, detail: detail.into().replace(['\n', '\r'], " ") }
}

/// The lines to print, checks first and the RESULT line last.
pub fn lines(mode: &str, checks: &[Check]) -> Vec<String> {
    let mut v: Vec<String> = checks.iter().map(|c| format!("IYAKKI-SELFTEST {mode} check {} {} {}", c.name, if c.ok { "PASS" } else { "FAIL" }, c.detail)).collect();
    let passed = checks.iter().filter(|c| c.ok).count();
    let all = !checks.is_empty() && passed == checks.len();
    v.push(format!("IYAKKI-SELFTEST {mode} RESULT {} {passed}/{}", if all { "PASS" } else { "FAIL" }, checks.len()));
    v
}

/// What a log says about one mode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// (name, passed) in log order.
    pub checks: Vec<(String, bool)>,
    /// (passed, passes, total) of the last RESULT line.
    pub result: Option<(bool, u32, u32)>,
}

/// Read the lines of `mode` out of a serial log (carriage returns and other lines are ignored).
pub fn parse(log: &str, mode: &str) -> Parsed {
    let (check_pre, result_pre) = (format!("IYAKKI-SELFTEST {mode} check "), format!("IYAKKI-SELFTEST {mode} RESULT "));
    let mut p = Parsed::default();
    for raw in log.lines() {
        let l = raw.trim_end_matches('\r');
        let l = l.find("IYAKKI-SELFTEST").map_or(l, |i| &l[i..]);
        if let Some(rest) = l.strip_prefix(&check_pre) {
            let mut it = rest.split_whitespace();
            if let (Some(name), Some(v)) = (it.next(), it.next()) {
                if v == "PASS" || v == "FAIL" {
                    p.checks.push((name.to_string(), v == "PASS"));
                }
            }
        } else if let Some(rest) = l.strip_prefix(&result_pre) {
            let mut it = rest.split_whitespace();
            if let (Some(v), Some(frac)) = (it.next(), it.next()) {
                if let Some((a, b)) = frac.split_once('/') {
                    if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
                        p.result = Some((v == "PASS", a, b));
                    }
                }
            }
        }
    }
    p
}

/// `Ok` only for a RESULT PASS that agrees with the checks printed before it.
pub fn verdict(p: &Parsed) -> Result<(), String> {
    let Some((pass, a, b)) = p.result else { return Err("no RESULT line (the guest did not finish)".into()) };
    let failed: Vec<&str> = p.checks.iter().filter(|c| !c.1).map(|c| c.0.as_str()).collect();
    let passes = p.checks.iter().filter(|c| c.1).count() as u32;
    if !failed.is_empty() {
        return Err(format!("failed: {}", failed.join(", ")));
    }
    if !pass {
        return Err(format!("RESULT FAIL {a}/{b}"));
    }
    if passes != a || p.checks.len() as u32 != b || b == 0 {
        return Err(format!("RESULT {a}/{b} does not match the {passes} of {} checks printed", p.checks.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Check> {
        vec![check("table loaded", true, "iyakki_sbx_0007"), check("blocked-refused", true, "timed out"), check("teardown", true, "no veth\nno table")]
    }

    #[test]
    fn lines_are_checks_then_one_result_and_parse_reads_them_back_through_serial_noise() {
        let l = lines("sandboxnet", &sample());
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], "IYAKKI-SELFTEST sandboxnet check table-loaded PASS iyakki_sbx_0007");
        assert_eq!(l[2], "IYAKKI-SELFTEST sandboxnet check teardown PASS no veth no table", "a detail is one line");
        assert_eq!(l[3], "IYAKKI-SELFTEST sandboxnet RESULT PASS 3/3");
        let log = format!("[    1.2] kernel noise\r\n{}\r\nIYAKKI-SELFTEST other check x FAIL no\r\nlogin: ", l.iter().map(|x| format!("[   9.9] {x}")).collect::<Vec<_>>().join("\r\n"));
        let p = parse(&log, "sandboxnet");
        assert_eq!(p.checks.len(), 3);
        assert_eq!(p.result, Some((true, 3, 3)));
        assert_eq!(verdict(&p), Ok(()));
        assert!(parse(&log, "other").checks == vec![("x".to_string(), false)], "modes do not mix");
    }

    #[test]
    fn a_failed_check_a_missing_result_a_lying_result_and_an_empty_run_are_all_failures() {
        let mut c = sample();
        c[1].ok = false;
        let l = lines("m", &c);
        assert_eq!(l.last().unwrap(), "IYAKKI-SELFTEST m RESULT FAIL 2/3");
        let e = verdict(&parse(&l.join("\n"), "m")).unwrap_err();
        assert!(e.contains("blocked-refused"), "{e}");
        let cut = lines("m", &sample())[..2].join("\n");
        assert!(verdict(&parse(&cut, "m")).unwrap_err().contains("no RESULT"));
        let liar = "IYAKKI-SELFTEST m check a PASS x\nIYAKKI-SELFTEST m RESULT PASS 2/2";
        assert!(verdict(&parse(liar, "m")).unwrap_err().contains("does not match"));
        let empty = lines("m", &[]);
        assert_eq!(empty, vec!["IYAKKI-SELFTEST m RESULT FAIL 0/0"]);
        assert!(verdict(&parse(&empty.join("\n"), "m")).is_err());
        assert!(verdict(&parse("", "m")).is_err());
        // garbage after the markers does not become a result
        assert_eq!(parse("IYAKKI-SELFTEST m RESULT PASS lots", "m").result, None);
        assert_eq!(parse("IYAKKI-SELFTEST m check onlyname", "m").checks, vec![]);
    }
}
