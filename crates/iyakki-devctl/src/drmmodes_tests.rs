use super::*;
use std::path::PathBuf;

/// A detailed timing descriptor from sizes and blanking: 18 bytes.
fn dtd_bytes(clock_10khz: u16, ha: u32, hb: u32, va: u32, vb: u32) -> [u8; 18] {
    let mut d = [0u8; 18];
    d[0..2].copy_from_slice(&clock_10khz.to_le_bytes());
    d[2] = ha as u8;
    d[3] = hb as u8;
    d[4] = (((ha >> 8) as u8) << 4) | ((hb >> 8) as u8);
    d[5] = va as u8;
    d[6] = vb as u8;
    d[7] = (((va >> 8) as u8) << 4) | ((vb >> 8) as u8);
    d
}

fn seal(block: &mut [u8]) {
    let sum: u32 = block[..127].iter().map(|b| u32::from(*b)).sum();
    block[127] = (256 - (sum % 256) as u32) as u8;
}

/// The monitor of the Mac mini report (maker SGT, product 0x1730): the base block with the given first timing and a CTA extension
/// with the video modes of that monitor and a 1920x1080 timing.
fn monitor_edid(preferred_4k: bool) -> Vec<u8> {
    let mut b = vec![0u8; 256];
    b[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    b[8..12].copy_from_slice(&[0x4c, 0xf4, 0x30, 0x17]);
    b[18] = 1;
    b[19] = 4;
    let first = if preferred_4k { dtd_bytes(53312, 3840, 160, 2160, 62) } else { dtd_bytes(14850, 1920, 280, 1080, 45) };
    b[54..72].copy_from_slice(&first);
    b[126] = 1;
    seal(&mut b[..128]);
    let vics: [u8; 18] = [16 | 0x80, 1, 2, 3, 4, 19, 31, 32, 33, 34, 60, 61, 62, 76, 93, 94, 95, 96];
    let c = &mut b[128..];
    c[0] = 0x02;
    c[1] = 0x03;
    c[2] = 4 + 1 + 18;
    c[4] = 0x40 | 18;
    c[5..23].copy_from_slice(&vics);
    c[23..41].copy_from_slice(&dtd_bytes(14850, 1920, 280, 1080, 45));
    seal(c);
    b
}

const FALLBACK: &str = "1920x1080\n1920x1080\n1920x1080\n1400x1050\n1280x1024\n1440x900\n1280x960\n1280x720\n1280x720\n1024x768\n800x600\n720x480\n640x480\n";
const FOUR_K: &str = "3840x2160\n3840x2160\n3840x2160\n2560x1440\n1920x1080\n1920x1080\n1280x720\n720x480\n640x480\n";

/// A fake /sys/class/drm with one connected connector.
fn fake(tag: &str, card: u32, edid: &[u8], modes: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-drmmodes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let c = d.join(format!("card{card}-DP-1"));
    std::fs::create_dir_all(&c).unwrap();
    std::fs::write(c.join("status"), "connected\n").unwrap();
    std::fs::write(c.join("edid"), edid).unwrap();
    std::fs::write(c.join("modes"), modes).unwrap();
    d
}

fn status(d: &Path, card: u32) -> String {
    std::fs::read_to_string(d.join(format!("card{card}-DP-1/status"))).unwrap().trim().to_string()
}

/// The fake kernel: after `needed` writes of `detect` it reads the monitor properly (EDID and modes become right).
struct Kernel {
    dir: PathBuf,
    card: u32,
    needed: Option<u32>,
    seen: u32,
    waits: Vec<Duration>,
}

impl Kernel {
    fn wait(&mut self, t: Duration) {
        self.waits.push(t);
        let p = self.dir.join(format!("card{}-DP-1", self.card));
        if std::fs::read_to_string(p.join("status")).unwrap().trim() == "detect" {
            self.seen += 1;
            std::fs::write(p.join("status"), "connected\n").unwrap();
            if self.needed.is_some_and(|n| self.seen >= n) {
                std::fs::write(p.join("edid"), monitor_edid(true)).unwrap();
                std::fs::write(p.join("modes"), FOUR_K).unwrap();
            }
        }
    }
}

fn run(k: &mut Kernel, force: bool, lines: &mut Vec<String>) -> Vec<Recovery> {
    let dir = k.dir.clone();
    let cell = std::cell::RefCell::new(k);
    recover(&dir, "test", force, &RETRY_DELAYS, &mut |t| cell.borrow_mut().wait(t), &mut |l| lines.push(l.to_string()))
}

#[test]
fn the_fixture_is_the_monitor_of_the_report() {
    let e = monitor_edid(true);
    assert_eq!(&e[..8], &[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    assert_eq!(e[..128].iter().map(|b| u32::from(*b)).sum::<u32>() % 256, 0, "the base checksum is right");
    assert_eq!(e[128..].iter().map(|b| u32::from(*b)).sum::<u32>() % 256, 0, "the CTA checksum is right");
    let i = parse_edid(&e).unwrap();
    assert_eq!(i.preferred, Some((3840, 2160, 60)), "533.12 MHz over 4000 x 2222 is 60 Hz");
    assert_eq!(i.sizes[0], (3840, 2160));
    for s in [(1920, 1080), (1280, 720), (720, 480), (640, 480)] {
        assert!(i.sizes.contains(&s), "{s:?} in {:?}", i.sizes);
    }
    // the same monitor answering with the 1080p timing first still names 4K through its video data block
    let i = parse_edid(&monitor_edid(false)).unwrap();
    assert_eq!(i.preferred, Some((1920, 1080, 60)));
    assert!(i.sizes.contains(&(3840, 2160)));
    // not an EDID
    assert_eq!(parse_edid(&[]), None);
    assert_eq!(parse_edid(&[0u8; 128]), None);
    assert_eq!(parse_edid(&e[..100]), None);
}

#[test]
fn the_state_of_the_report_is_found_and_good_lists_are_left_alone() {
    let e = monitor_edid(true);
    assert_eq!(assess(&e, FALLBACK), Health::MissingPreferred(3840, 2160), "the fallback list of the report");
    assert_eq!(assess(&e, FOUR_K), Health::Fine);
    assert_eq!(assess(&e, ""), Health::NoModes);
    assert_eq!(assess(&[], FALLBACK), Health::NoEdid, "an empty EDID file");
    assert_eq!(assess(&e[..128 - 1], FALLBACK), Health::NoEdid, "a short one");
    // preferred 1080p but the video data block names 4K and only fallback sizes are listed
    assert_eq!(assess(&monitor_edid(false), FALLBACK), Health::FallbackOnly(3840, 2160));
    // a plain 1080p monitor with 1080p modes is fine
    assert_eq!(assess(&monitor_edid(false), "1920x1080\n1280x720\n3840x2160\n"), Health::Fine);
    // modes with an interlace letter or a refresh suffix still count
    assert_eq!(assess(&e, "3840x2160p-60\n1920x1080i\n"), Health::Fine);
}

#[test]
fn stale_fallback_modes_become_4k_after_one_detect_and_nothing_else_is_touched() {
    let d = fake("stale", 10, &monitor_edid(true), FALLBACK);
    let mut k = Kernel { dir: d.clone(), card: 10, needed: Some(1), seen: 0, waits: vec![] };
    let mut lines = Vec::new();
    let r = run(&mut k, false, &mut lines);
    assert_eq!(r, vec![Recovery { connector: "DP-1".into(), attempts: 1, fixed: true }]);
    assert_eq!(k.waits, vec![Duration::from_secs(1)], "one second after the first detect, no more rounds");
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("iyakki.display_retry connector=DP-1") && lines[0].contains("attempt=1/3") && lines[0].contains("preferred 3840x2160 missing"), "{}", lines[0]);
    assert!(lines[0].contains("after=\"fine\""), "{}", lines[0]);
    assert!(std::fs::read_to_string(d.join("card10-DP-1/modes")).unwrap().starts_with("3840x2160"));
    assert_eq!(status(&d, 10), "connected");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_slow_kernel_gets_the_longer_waits_one_two_then_three() {
    let d = fake("slow", 11, &monitor_edid(true), FALLBACK);
    let mut k = Kernel { dir: d.clone(), card: 11, needed: Some(3), seen: 0, waits: vec![] };
    let mut lines = Vec::new();
    let r = run(&mut k, false, &mut lines);
    assert_eq!(r[0].attempts, 3);
    assert!(r[0].fixed);
    assert_eq!(k.waits, vec![Duration::from_secs(1), Duration::from_secs(3), Duration::from_secs(8)]);
    assert_eq!(lines.len(), 3);
    assert!(lines[2].contains("attempt=3/3") && lines[2].contains("waited_ms=8000"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_empty_edid_is_asked_again_until_the_monitor_answers() {
    let d = fake("noedid", 12, &[], FALLBACK);
    let mut k = Kernel { dir: d.clone(), card: 12, needed: Some(2), seen: 0, waits: vec![] };
    let mut lines = Vec::new();
    let r = run(&mut k, false, &mut lines);
    assert_eq!(r, vec![Recovery { connector: "DP-1".into(), attempts: 2, fixed: true }]);
    assert!(lines[0].contains("before=\"no usable EDID\"") && lines[0].contains("after=\"no usable EDID\""), "{}", lines[0]);
    assert!(lines[1].contains("after=\"fine\""));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_connector_that_is_right_gets_no_detect_and_no_wait() {
    let d = fake("fine", 13, &monitor_edid(true), FOUR_K);
    let mut k = Kernel { dir: d.clone(), card: 13, needed: None, seen: 0, waits: vec![] };
    let mut lines = Vec::new();
    assert!(run(&mut k, false, &mut lines).is_empty());
    assert!(k.waits.is_empty() && lines.is_empty());
    assert_eq!(status(&d, 13), "connected", "nothing was written");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_monitor_that_never_answers_is_tried_three_times_then_left_alone_until_look_again() {
    let d = fake("never", 14, &monitor_edid(true), FALLBACK);
    let mut k = Kernel { dir: d.clone(), card: 14, needed: None, seen: 0, waits: vec![] };
    let mut lines = Vec::new();
    let r = run(&mut k, false, &mut lines);
    assert_eq!(r, vec![Recovery { connector: "DP-1".into(), attempts: 3, fixed: false }]);
    assert_eq!(k.waits.iter().sum::<Duration>(), Duration::from_secs(12), "bounded: 1 + 3 + 8 seconds");
    assert!(lines.last().unwrap().contains("gave up after 3 attempts"));
    // the event our own detect caused arrives: the same state is not tried again
    let before = k.seen;
    assert!(run(&mut k, false, &mut lines).is_empty());
    assert_eq!(k.seen, before);
    // "Look again" forgets the give-up
    let r = run(&mut k, true, &mut lines);
    assert_eq!(r[0].attempts, 3);
    assert_eq!(k.seen, before + 3);
    // a changed EDID or mode list is a new state
    std::fs::write(d.join("card14-DP-1/modes"), "1920x1080\n1280x720\n").unwrap();
    let r = run(&mut k, false, &mut lines);
    assert_eq!(r[0].attempts, 3);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unplugged_monitor_ends_the_retry() {
    let d = fake("gone", 15, &monitor_edid(true), FALLBACK);
    let dir = d.clone();
    let mut lines = Vec::new();
    let r = recover(&d, "test", false, &RETRY_DELAYS, &mut |_| std::fs::write(dir.join("card15-DP-1/status"), "disconnected\n").unwrap(), &mut |l| lines.push(l.to_string()));
    assert_eq!(r, vec![Recovery { connector: "DP-1".into(), attempts: 1, fixed: false }]);
    assert!(lines[0].contains("after=\"disconnected\""));
}

#[test]
fn only_connected_connectors_of_cards_are_judged_and_the_status_line_says_how_they_look() {
    let d = fake("status", 17, &monitor_edid(true), FALLBACK);
    std::fs::create_dir_all(d.join("card17-HDMI-A-1")).unwrap();
    std::fs::write(d.join("card17-HDMI-A-1/status"), "disconnected\n").unwrap();
    std::fs::create_dir_all(d.join("version")).unwrap();
    assert_eq!(check_all(&d).len(), 1);
    assert_eq!(status_text(&d), "DP-1: preferred 3840x2160 missing from the modes");
    std::fs::write(d.join("card17-DP-1/modes"), FOUR_K).unwrap();
    assert_eq!(status_text(&d), "DP-1: mode list matches the display");
    assert_eq!(status_text(&d.join("none")), "No connected display was found.");
    let _ = std::fs::remove_dir_all(&d);
}
