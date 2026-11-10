use super::*;
use iyakki_proxy::StreamHead;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Mutex;

const BOARD: &str = "Mac-7BA5B2DFE22DDD8C";
const IMG: &str = "http://oscdn.apple.com/Apple/macOS/BaseSystem.dmg";
const CHK: &str = "http://oscdn.apple.com/Apple/macOS/BaseSystem.chunklist";

fn tmp(tag: &str) -> PathBuf {
    let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-macrec-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn policy() -> Policy {
    Policy { retry_pause: std::time::Duration::ZERO, ..Policy::default() }
}

/// An image of `n` bytes with a chunk list over it (chunk size `cs`).
fn image_and_list(n: usize, cs: usize) -> (Vec<u8>, Vec<u8>) {
    let img: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
    (img.clone(), chunklist_for(&img, cs))
}

fn chunklist_for(img: &[u8], cs: usize) -> Vec<u8> {
    let chunks: Vec<&[u8]> = img.chunks(cs).collect();
    let mut b = Vec::new();
    b.extend_from_slice(b"CNKL");
    b.extend_from_slice(&36u32.to_le_bytes());
    b.extend_from_slice(&[1, 1, 1, 0]);
    b.extend_from_slice(&(chunks.len() as u64).to_le_bytes());
    b.extend_from_slice(&36u64.to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes());
    for c in chunks {
        b.extend_from_slice(&(c.len() as u32).to_le_bytes());
        b.extend_from_slice(&Sha256::digest(c));
    }
    b
}

struct Flaky {
    data: Vec<u8>,
    pos: usize,
    fail_at: Option<usize>,
}

impl Read for Flaky {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.fail_at.is_some_and(|f| self.pos >= f) {
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset by peer"));
        }
        let end = self.fail_at.unwrap_or(self.data.len()).min(self.data.len());
        let n = out.len().min(end - self.pos).min(100_000);
        out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// A fake of Apple's service: the cookie, the POST, and the asset downloads with `Range`.
struct FakeApple {
    image: Vec<u8>,
    chunklist: Vec<u8>,
    info: Mutex<String>,
    /// Answer every request with a full 200 (a server that ignores Range).
    ignore_range: bool,
    /// The first download attempt breaks after this many bytes.
    drop_first_at: Mutex<Option<usize>>,
    /// Redirect the image to this address.
    redirect_image: Option<String>,
    image_status: u16,
    log: Mutex<Vec<(String, String, Vec<(String, String)>, String)>>,
}

impl FakeApple {
    fn new(image: Vec<u8>, chunklist: Vec<u8>) -> FakeApple {
        let info = format!("AP: 012-34567\nAU: {IMG}\nAH: {}\nAT: IMGTOKEN\nCU: {CHK}\nCH: {}\nCT: CHKTOKEN\n", "0".repeat(64), "1".repeat(64));
        FakeApple { image, chunklist, info: Mutex::new(info), ignore_range: false, drop_first_at: Mutex::new(None), redirect_image: None, image_status: 200, log: Mutex::new(vec![]) }
    }
    fn calls(&self) -> Vec<(String, String, Vec<(String, String)>, String)> {
        self.log.lock().unwrap().clone()
    }
    fn header(h: &[(String, String)], k: &str) -> Option<String> {
        h.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone())
    }
}

impl Streamer for FakeApple {
    fn open(&self, url: &str, h: &[(String, String)]) -> Result<StreamHead, String> {
        self.log.lock().unwrap().push(("GET".into(), url.into(), h.to_vec(), String::new()));
        let head = |status: u16, headers: Vec<(&str, String)>, body: Box<dyn Read + Send>| Ok(StreamHead { status, headers: headers.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), body });
        if url == "http://osrecovery.apple.com/" {
            return head(200, vec![("set-cookie", "session=abc123; Path=/; HttpOnly".into())], Box::new(Cursor::new(Vec::new())));
        }
        let moved = self.redirect_image.as_ref().is_some_and(|r| r.starts_with('/') && url == format!("http://oscdn.apple.com{r}"));
        if url == IMG || moved {
            if Self::header(h, "cookie").as_deref() != Some("AssetToken=IMGTOKEN") {
                return head(403, vec![], Box::new(Cursor::new(Vec::new())));
            }
            if let (true, Some(r)) = (url == IMG, &self.redirect_image) {
                return head(302, vec![("location", r.clone())], Box::new(Cursor::new(Vec::new())));
            }
            if self.image_status != 200 {
                return head(self.image_status, vec![], Box::new(Cursor::new(Vec::new())));
            }
            let from: usize = if self.ignore_range { 0 } else { Self::header(h, "range").and_then(|r| r.strip_prefix("bytes=")?.trim_end_matches('-').parse().ok()).unwrap_or(0) };
            let total = self.image.len();
            if from > total || (from == total && from > 0) {
                return head(416, vec![("content-range", format!("bytes */{total}"))], Box::new(Cursor::new(Vec::new())));
            }
            let fail = self.drop_first_at.lock().unwrap().take();
            let body = Flaky { data: self.image[from..].to_vec(), pos: 0, fail_at: fail };
            return if from > 0 { head(206, vec![("content-range", format!("bytes {from}-{}/{total}", total - 1)), ("content-length", (total - from).to_string())], Box::new(body)) } else { head(200, vec![("content-length", total.to_string())], Box::new(body)) };
        }
        if url == CHK {
            if Self::header(h, "cookie").as_deref() != Some("AssetToken=CHKTOKEN") {
                return head(403, vec![], Box::new(Cursor::new(Vec::new())));
            }
            return head(200, vec![("content-length", self.chunklist.len().to_string())], Box::new(Cursor::new(self.chunklist.clone())));
        }
        Err(format!("no route for {url}"))
    }

    fn post(&self, url: &str, h: &[(String, String)], body: &[u8]) -> Result<StreamHead, String> {
        self.log.lock().unwrap().push(("POST".into(), url.into(), h.to_vec(), String::from_utf8_lossy(body).into_owned()));
        if url != "http://osrecovery.apple.com/InstallationPayload/RecoveryImage" {
            return Err(format!("no route for {url}"));
        }
        if Self::header(h, "cookie").as_deref() != Some("session=abc123") {
            return Ok(StreamHead { status: 403, headers: vec![], body: Box::new(Cursor::new(b"no session".to_vec())) });
        }
        let text = String::from_utf8_lossy(body);
        if !text.contains(&format!("bid={BOARD}")) {
            return Ok(StreamHead { status: 404, headers: vec![], body: Box::new(Cursor::new(Vec::new())) });
        }
        Ok(StreamHead { status: 200, headers: vec![], body: Box::new(Cursor::new(self.info.lock().unwrap().clone().into_bytes())) })
    }
}

#[test]
fn the_session_cookie_is_the_name_and_value_before_the_attributes() {
    let f = FakeApple::new(vec![], vec![]);
    assert_eq!(get_session(&f, "http://osrecovery.apple.com").unwrap(), "session=abc123");
    let c = f.calls();
    assert_eq!(c[0].1, "http://osrecovery.apple.com/");
    assert_eq!(FakeApple::header(&c[0].2, "user-agent").as_deref(), Some("InternetRecovery/1.0"));
}

#[test]
fn a_service_that_sends_no_cookie_or_an_error_is_reported_plainly() {
    struct NoCookie(u16);
    impl Streamer for NoCookie {
        fn open(&self, _u: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            Ok(StreamHead { status: self.0, headers: vec![], body: Box::new(Cursor::new(vec![])) })
        }
    }
    assert!(get_session(&NoCookie(200), "http://x").unwrap_err().contains("no session cookie"));
    assert!(get_session(&NoCookie(503), "http://x").unwrap_err().contains("HTTP 503"));
}

#[test]
fn the_request_carries_cookie_and_the_six_fields_in_order() {
    let f = FakeApple::new(vec![], vec![]);
    let ids = Ids { cid: "A".repeat(16), k: "B".repeat(64), fg: "C".repeat(64) };
    let info = get_image_info(&f, "http://osrecovery.apple.com", "session=abc123", BOARD, MLB_ZERO, "default", &ids).unwrap();
    assert_eq!(info.image_url, IMG);
    assert_eq!((info.image_token.as_str(), info.chunklist_token.as_str(), info.product.as_str()), ("IMGTOKEN", "CHKTOKEN", "012-34567"));
    let post = f.calls().into_iter().find(|c| c.0 == "POST").unwrap();
    assert_eq!(post.3, format!("cid={}\nsn={MLB_ZERO}\nbid={BOARD}\nk={}\nfg={}\nos=default", "A".repeat(16), "B".repeat(64), "C".repeat(64)));
    assert_eq!(FakeApple::header(&post.2, "content-type").as_deref(), Some("text/plain"));
    assert_eq!(FakeApple::header(&post.2, "cookie").as_deref(), Some("session=abc123"));
}

#[test]
fn random_ids_have_the_right_lengths_and_differ() {
    let (a, b) = (Ids::random(), Ids::random());
    assert_eq!((a.cid.len(), a.k.len(), a.fg.len()), (16, 64, 64));
    assert!(a.cid.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
    assert_ne!(a, b);
}

#[test]
fn a_wrong_board_session_or_answer_is_a_plain_error() {
    let f = FakeApple::new(vec![], vec![]);
    let ids = Ids::random();
    assert!(get_image_info(&f, "http://osrecovery.apple.com", "session=abc123", "Mac-nope", MLB_ZERO, "default", &ids).unwrap_err().contains("not a Mac board id"));
    assert!(get_image_info(&f, "http://osrecovery.apple.com", "session=wrong", BOARD, MLB_ZERO, "default", &ids).unwrap_err().contains("HTTP 403"));
    assert!(get_image_info(&f, "http://osrecovery.apple.com", "session=abc123", "Mac-0000000000000000", MLB_ZERO, "default", &ids).unwrap_err().contains("no recovery image for board"));
    *f.info.lock().unwrap() = "AP: x\nAU: http://oscdn.apple.com/a\n".into();
    let e = get_image_info(&f, "http://osrecovery.apple.com", "session=abc123", BOARD, MLB_ZERO, "default", &ids).unwrap_err();
    assert!(e.contains("did not send AH") || e.contains("did not send AT"), "{e}");
    assert!(parse_info("garbage").unwrap_err().contains("did not send"));
}

#[test]
fn only_apple_hosts_are_allowed_including_their_subdomains_and_never_look_alikes() {
    let p = Policy::default();
    assert!(p.allows("http://oscdn.apple.com/x") && p.allows("https://apple.com/x") && p.allows("http://osrecovery.apple.com:80/x"));
    for bad in ["http://evilapple.com/x", "http://apple.com.evil.example/x", "http://user@apple.com/x", "ftp://apple.com/x", "http://127.0.0.1/x", "apple.com", "http://apple.com@evil.example/"] {
        assert!(!p.allows(bad), "{bad}");
    }
    assert_eq!(host_of("http://Host.Example:8080/a?b#c").as_deref(), Some("host.example"));
}

#[test]
fn content_ranges_are_parsed() {
    assert_eq!(parse_content_range("bytes 100-199/200"), Some((100, 199, Some(200))));
    assert_eq!(parse_content_range("bytes 0-9/*"), Some((0, 9, None)), "an unknown total is allowed");
    assert_eq!(parse_content_range("items 1-2/3"), None);
}

#[test]
fn a_whole_fetch_downloads_the_image_checks_every_chunk_and_says_each_step() {
    let (img, list) = image_and_list(1_000_003, 100_000);
    let f = FakeApple::new(img.clone(), list);
    let dir = tmp("whole");
    let mut said = Vec::new();
    let p = fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |l| said.push(l.to_string()), &|| false, 2).unwrap();
    assert_eq!(p, dir.join("BaseSystem.dmg"));
    assert_eq!(std::fs::read(&p).unwrap(), img);
    let all = said.join("\n");
    for w in ["session", "board Mac-7BA5B2DFE22DDD8C", "chunk list", "1000003 bytes in 11 chunks", "downloading", "checking the image", "every chunk matches"] {
        assert!(all.contains(w), "{w}: {all}");
    }
    assert!(all.contains("does not match the AH value"), "AH is advisory: {all}");
    // the order of the calls: cookie, POST, chunk list, image
    let urls: Vec<String> = f.calls().into_iter().map(|c| c.1).collect();
    assert_eq!(urls[0], "http://osrecovery.apple.com/");
    assert!(urls[1].ends_with("/InstallationPayload/RecoveryImage") && urls[2] == CHK && urls[3] == IMG);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_download_that_stopped_halfway_resumes_from_the_bytes_on_disk() {
    let (img, list) = image_and_list(500_000, 100_000);
    let f = FakeApple::new(img.clone(), list);
    let dir = tmp("resume");
    std::fs::write(dir.join("BaseSystem.dmg"), &img[..200_000]).unwrap();
    std::fs::write(dir.join("BaseSystem.dmg.id"), format!("{IMG} {}", "0".repeat(64))).unwrap(); // the part is of this image
    let mut said = Vec::new();
    fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |l| said.push(l.to_string()), &|| false, 2).unwrap();
    assert_eq!(std::fs::read(dir.join("BaseSystem.dmg")).unwrap(), img);
    let img_req = f.calls().into_iter().filter(|c| c.1 == IMG).last().unwrap();
    assert_eq!(FakeApple::header(&img_req.2, "range").as_deref(), Some("bytes=200000-"));
    assert!(said.iter().any(|l| l.contains("resumed")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_part_of_another_image_is_never_continued() {
    let (img, list) = image_and_list(500_000, 100_000);
    let f = FakeApple::new(img.clone(), list);
    let dir = tmp("stale");
    let other: Vec<u8> = img.iter().map(|b| b ^ 0x5a).collect();
    std::fs::write(dir.join("BaseSystem.dmg"), &other[..200_000]).unwrap();
    std::fs::write(dir.join("BaseSystem.dmg.id"), "http://oscdn.apple.com/older/BaseSystem.dmg FFFF").unwrap(); // another image
    let mut said = Vec::new();
    fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |l| said.push(l.to_string()), &|| false, 2).unwrap();
    assert_eq!(std::fs::read(dir.join("BaseSystem.dmg")).unwrap(), img);
    assert!(said.iter().any(|l| l.contains("not this image")), "{said:?}");
    assert!(f.calls().iter().filter(|c| c.1 == IMG).all(|c| FakeApple::header(&c.2, "range").is_none()), "downloaded from the start");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_wrong_part_without_a_marker_is_caught_by_the_check_and_the_image_is_fetched_whole_once_more() {
    let (img, list) = image_and_list(500_000, 100_000);
    let f = FakeApple::new(img.clone(), list);
    let dir = tmp("stale-nomarker");
    let other: Vec<u8> = img.iter().map(|b| b ^ 0x5a).collect();
    std::fs::write(dir.join("BaseSystem.dmg"), &other[..200_000]).unwrap();
    let mut said = Vec::new();
    fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |l| said.push(l.to_string()), &|| false, 2).unwrap();
    assert_eq!(std::fs::read(dir.join("BaseSystem.dmg")).unwrap(), img);
    assert!(said.iter().any(|l| l.contains("once more")), "{said:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_server_message_instead_of_the_image_is_named_and_tried_once_more() {
    let text = b"<html><body>Access Denied</body></html>".to_vec();
    let mut img = text.clone();
    img.resize(300_000, b' ');
    let (_, list) = image_and_list(300_000, 100_000); // a chunk list of a different file
    let f = FakeApple::new(img, list);
    let dir = tmp("html");
    let mut said = Vec::new();
    let e = fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |l| said.push(l.to_string()), &|| false, 0).unwrap_err();
    assert!(e.contains("chunk 1 of 3") && e.contains("starts with the text") && e.contains("<html>"), "{e}");
    assert!(said.iter().any(|l| l.contains("once more")), "{said:?}");
    assert_eq!(f.calls().iter().filter(|c| c.1 == IMG).count(), 2, "the image was asked for twice");
    assert!(!dir.join("BaseSystem.dmg").exists());
    assert!(start_of(&[0x78, 0xda, 0x63, 0x60, 0x18]).contains("78 da 63"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_server_that_ignores_range_restarts_the_file_instead_of_appending_garbage() {
    let (img, list) = image_and_list(300_000, 100_000);
    let mut f = FakeApple::new(img.clone(), list);
    f.ignore_range = true;
    let dir = tmp("norange");
    std::fs::write(dir.join("BaseSystem.dmg"), b"old partial bytes").unwrap();
    fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 2).unwrap();
    assert_eq!(std::fs::read(dir.join("BaseSystem.dmg")).unwrap(), img);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dropped_connection_is_resumed_automatically() {
    let (img, list) = image_and_list(600_000, 100_000);
    let f = FakeApple::new(img.clone(), list);
    *f.drop_first_at.lock().unwrap() = Some(250_000);
    let dir = tmp("drop");
    fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 3).unwrap();
    assert_eq!(std::fs::read(dir.join("BaseSystem.dmg")).unwrap(), img);
    let gets: Vec<_> = f.calls().into_iter().filter(|c| c.1 == IMG).collect();
    assert_eq!(gets.len(), 2);
    assert!(FakeApple::header(&gets[1].2, "range").unwrap().starts_with("bytes="), "the second try asks for the rest");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_retries_a_dropped_connection_fails_and_keeps_the_part() {
    let (img, list) = image_and_list(600_000, 100_000);
    let f = FakeApple::new(img, list);
    *f.drop_first_at.lock().unwrap() = Some(250_000);
    let dir = tmp("noretry");
    let e = fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 0).unwrap_err();
    assert!(e.contains("did not finish") && e.contains("connection dropped"), "{e}");
    assert_eq!(std::fs::metadata(dir.join("BaseSystem.dmg")).unwrap().len(), 250_000, "kept for the next try");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_complete_file_on_disk_is_accepted_when_the_server_says_the_range_is_past_the_end() {
    let (img, list) = image_and_list(250_000, 100_000);
    let f = FakeApple::new(img.clone(), list);
    let dir = tmp("complete");
    std::fs::write(dir.join("BaseSystem.dmg"), &img).unwrap();
    fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 1).unwrap();
    assert_eq!(std::fs::read(dir.join("BaseSystem.dmg")).unwrap(), img);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_damaged_image_fails_the_chunk_check_and_is_deleted() {
    let (mut img, list) = image_and_list(400_000, 100_000);
    let good = img.clone();
    img[250_000] ^= 0xff;
    let f = FakeApple::new(img, list);
    let dir = tmp("bad");
    let e = fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 1).unwrap_err();
    assert!(e.contains("chunk 3 of 4") && e.contains("does not match"), "{e}");
    assert!(!dir.join("BaseSystem.dmg").exists(), "a damaged file is never resumed from");
    drop(good);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_short_image_fails_on_size_before_any_hashing() {
    let (img, list) = image_and_list(400_000, 100_000);
    let dir = tmp("short");
    std::fs::write(dir.join("a.dmg"), &img[..399_999]).unwrap();
    let e = verify_image(&dir.join("a.dmg"), &parse_chunklist(&list).unwrap(), &mut |_, _| {}, &|| false).unwrap_err();
    assert!(e.contains("399999") && e.contains("400000"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn addresses_outside_apple_are_refused_in_the_answer_and_in_redirects() {
    let (img, list) = image_and_list(100_000, 50_000);
    let f = FakeApple::new(img.clone(), list.clone());
    *f.info.lock().unwrap() = format!("AP: x\nAU: http://evil.example/BaseSystem.dmg\nAH: h\nAT: t\nCU: {CHK}\nCH: h\nCT: CHKTOKEN\n");
    let dir = tmp("foreign");
    let e = fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 0).unwrap_err();
    assert!(e.contains("not an Apple address"), "{e}");
    assert!(!f.calls().iter().any(|c| c.1.contains("evil.example")), "nothing was requested from it");
    let mut g = FakeApple::new(img, list);
    g.redirect_image = Some("http://evil.example/steal".into());
    let e = fetch_recovery_image(&g, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 0).unwrap_err();
    assert!(e.contains("not an Apple address"), "{e}");
    assert!(!g.calls().iter().any(|c| c.1.contains("evil.example")), "the token was not sent there");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_redirect_inside_apple_is_followed_with_the_token() {
    let (img, list) = image_and_list(100_000, 50_000);
    let mut f = FakeApple::new(img.clone(), list);
    f.redirect_image = Some("/Apple/macOS/moved/BaseSystem.dmg".into());
    let dir = tmp("redirect");
    fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 0).unwrap();
    assert_eq!(std::fs::read(dir.join("BaseSystem.dmg")).unwrap(), img);
    let moved = f.calls().into_iter().find(|c| c.1.ends_with("/moved/BaseSystem.dmg")).expect("followed");
    assert_eq!(FakeApple::header(&moved.2, "cookie").as_deref(), Some("AssetToken=IMGTOKEN"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_refused_download_says_the_token_may_have_expired_and_a_huge_one_is_refused() {
    let (img, list) = image_and_list(100_000, 50_000);
    let mut f = FakeApple::new(img.clone(), list.clone());
    f.image_status = 403;
    let dir = tmp("refused");
    let e = fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| false, 0).unwrap_err();
    assert!(e.contains("token may have expired"), "{e}");
    let g = FakeApple::new(img, list);
    let e = download(&g, &policy(), IMG, "IMGTOKEN", &dir.join("x.dmg"), 1000, 0, &mut |_, _| {}, &|| false).unwrap_err();
    assert!(e.contains("more than the 1000 allowed"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn stopping_the_job_stops_the_download_and_keeps_what_was_written() {
    let (img, list) = image_and_list(1_000_000, 100_000);
    let f = FakeApple::new(img, list);
    let dir = tmp("cancel");
    let n = std::cell::Cell::new(0);
    let e = fetch_recovery_image(&f, "http://osrecovery.apple.com", &policy(), BOARD, &dir, &mut |_| {}, &|| {
        n.set(n.get() + 1);
        n.get() > 3
    }, 0)
    .unwrap_err();
    assert_eq!(e, "stopped");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn chunk_lists_that_are_cut_short_or_foreign_are_refused() {
    let (_, list) = image_and_list(300_000, 100_000);
    assert!(parse_chunklist(&list).is_ok());
    assert!(parse_chunklist(&list[..list.len() - 1]).unwrap_err().contains("cut short"));
    assert!(parse_chunklist(b"XXXX").unwrap_err().contains("CNKL"));
    let mut bad = list.clone();
    bad[4] = 40;
    assert!(parse_chunklist(&bad).unwrap_err().contains("header size"));
    let c = parse_chunklist(&list).unwrap();
    assert_eq!((c.chunks.len(), c.total()), (3, 300_000));
}

#[test]
fn the_real_http_client_speaks_to_a_local_fake_server_with_cookie_post_and_range() {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    let (img, list) = image_and_list(300_000, 100_000);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (img2, list2) = (img.clone(), list.clone());
    let seen: std::sync::Arc<Mutex<Vec<String>>> = Default::default();
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { break };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut headers = HashMap::new();
            loop {
                let mut l = String::new();
                r.read_line(&mut l).unwrap();
                if l.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = l.trim().split_once(':') {
                    headers.insert(k.to_ascii_lowercase(), v.trim().to_string());
                }
            }
            let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            seen2.lock().unwrap().push(format!("{} | cookie={} | range={} | body={}", first.trim(), headers.get("cookie").cloned().unwrap_or_default(), headers.get("range").cloned().unwrap_or_default(), String::from_utf8_lossy(&body).replace('\n', ",")));
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (head, payload): (String, Vec<u8>) = if first.starts_with("GET / ") {
                ("HTTP/1.1 200 OK\r\nSet-Cookie: session=live1; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(), vec![])
            } else if first.starts_with("POST ") {
                let info = format!("AP: 1\nAU: http://127.0.0.1:{port}/img\nAH: x\nAT: IT\nCU: http://127.0.0.1:{port}/chk\nCH: x\nCT: CT\n");
                (format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", info.len()), info.into_bytes())
            } else if path == "/chk" {
                (format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", list2.len()), list2.clone())
            } else {
                let from: usize = headers.get("range").and_then(|r| r.strip_prefix("bytes=")?.trim_end_matches('-').parse().ok()).unwrap_or(0);
                if from > 0 {
                    (format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", img2.len() - 1, img2.len(), img2.len() - from), img2[from..].to_vec())
                } else {
                    (format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", img2.len()), img2.clone())
                }
            };
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&payload);
        }
    });
    let st = iyakki_proxy::https::UreqStreamer::default();
    let p = Policy { hosts: vec!["127.0.0.1".into()], max_redirects: 2, retry_pause: std::time::Duration::ZERO };
    let dir = tmp("tcp");
    std::fs::write(dir.join("BaseSystem.dmg"), &img[..120_000]).unwrap();
    let got = fetch_recovery_image(&st, &format!("http://127.0.0.1:{port}"), &p, BOARD, &dir, &mut |_| {}, &|| false, 1).unwrap();
    assert_eq!(std::fs::read(got).unwrap(), img);
    let log = seen.lock().unwrap().join("\n");
    assert!(log.contains("GET / HTTP/1.1") && log.contains("POST /InstallationPayload/RecoveryImage") && log.contains("cookie=session=live1"), "{log}");
    assert!(log.contains(&format!("body=cid=")) && log.contains(&format!("bid={BOARD}")), "{log}");
    assert!(log.contains("cookie=AssetToken=IT") && log.contains("range=bytes=120000-"), "{log}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_os_choice_goes_into_the_request_and_the_identity_tells_two_images_apart_without_downloading() {
    let f = FakeApple::new(vec![], vec![]);
    let id = image_identity(&f, "http://osrecovery.apple.com", &policy(), BOARD, "latest").unwrap();
    assert_eq!(id, format!("012-34567 {IMG}"));
    let post = f.calls().into_iter().find(|c| c.0 == "POST").unwrap();
    assert!(post.3.ends_with("os=latest"), "{}", post.3);
    assert!(!f.calls().iter().any(|c| c.1 == IMG), "nothing was downloaded");
    *f.info.lock().unwrap() = "AP: x\nAU: http://evil.example/a\nAH: h\nAT: t\nCU: http://oscdn.apple.com/c\nCH: h\nCT: t\n".into();
    assert!(image_identity(&f, "http://osrecovery.apple.com", &policy(), BOARD, "default").unwrap_err().contains("not an Apple address"));
}

#[test]
fn a_serial_part_chooses_the_macos_version_and_the_log_names_the_host_and_the_bytes() {
    let (img, list) = image_and_list(500_000, 100_000);
    let f = FakeApple::new(img.clone(), list);
    let dir = tmp("mlb");
    std::fs::write(dir.join("BaseSystem.dmg"), &img[..200_000]).unwrap();
    let mut said = Vec::new();
    fetch_recovery_image_for(&f, "http://osrecovery.apple.com", &policy(), BOARD, "00000000000KXPG00", "default", &dir, &mut |l| said.push(l.to_string()), &|| false, 2).unwrap();
    let post = f.calls().into_iter().find(|c| c.0 == "POST").unwrap();
    assert!(post.3.contains("\nsn=00000000000KXPG00\n") && post.3.contains("\nos=default"), "{}", post.3);
    let all = said.join("\n");
    for w in ["recovery service (osrecovery.apple.com)", "serial part 00000000000KXPG00", "image host", "downloaded: 500000 bytes on disk (300000 bytes came over the network)"] {
        assert!(all.contains(w), "{w}: {all}");
    }
    assert!(!all.contains("IMGTOKEN") && !all.contains("CHKTOKEN"), "no tokens in the log");
    // the identity question takes the same part
    let id = image_identity_for(&f, "http://osrecovery.apple.com", &policy(), BOARD, "00000000000KXPG00", "latest").unwrap();
    assert!(id.starts_with("012-34567 "), "{id}");
    assert!(f.calls().into_iter().filter(|c| c.0 == "POST").last().unwrap().3.contains("sn=00000000000KXPG00"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Live check against Apple's real service (run by hand: `cargo test -p iyakki-agent --lib live_first_chunk -- --ignored --nocapture`).
/// Asks for the image of a board, fetches the chunk list, downloads only the first chunk (the size cap stops the rest) and compares.
#[test]
#[ignore]
fn live_first_chunk_of_the_real_image_matches_apple_chunk_list() {
    let board = std::env::var("IYAKKI_LIVE_BOARD").unwrap_or_else(|_| "Mac-5F9802EFE386AA28".into());
    let st = iyakki_proxy::https::UreqStreamer::default();
    let cookie = get_session(&st, SERVICE).unwrap();
    let info = get_image_info(&st, SERVICE, &cookie, &board, MLB_ZERO, "default", &Ids::random()).unwrap();
    let list = fetch_chunklist(&st, &Policy::default(), &info).unwrap();
    let dir = tmp("live");
    let dest = dir.join("BaseSystem.dmg");
    let first = list.chunks[0].0 as u64;
    // the cap makes the download stop just after the first chunk (it reports an error, which is expected)
    let stop = std::cell::Cell::new(false);
    let r = download(&st, &Policy::default(), &info.image_url, &info.image_token, &dest, MAX_IMAGE, 0, &mut |d, _| if d >= first { stop.set(true) }, &|| stop.get());
    println!("download result: {r:?}; file {} bytes; product {}", std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0), info.product);
    let mut head = vec![0u8; first as usize];
    std::fs::File::open(&dest).unwrap().read_exact(&mut head).unwrap();
    assert_eq!(Sha256::digest(&head).as_slice(), list.chunks[0].1, "first chunk of the real image");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Live, whole image (about 0.9 GB downloaded and checked, then deleted): the complete path as the Mac drivers job runs it.
#[test]
#[ignore]
fn live_whole_image_downloads_and_verifies() {
    let board = std::env::var("IYAKKI_LIVE_BOARD").unwrap_or_else(|_| "Mac-5F9802EFE386AA28".into());
    let st = iyakki_proxy::https::UreqStreamer::default();
    let dir = tmp("live-full");
    let mut log = Vec::new();
    let r = fetch_recovery_image_for(&st, SERVICE, &Policy::default(), &board, MLB_ZERO, "default", &dir, &mut |l| { println!("{l}"); log.push(l.to_string()) }, &|| false, 3);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(r.is_ok(), "{r:?}");
}
