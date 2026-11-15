//! The PDF reader against files made and read by programs that are not ours: Ghostscript (`gs`, the writer, including its password
//! protection) and poppler's `pdftotext` (the second reader). Skipped, and said so, where either is missing (F-BROX-07, F-PDF-08). This Ghostscript (10.06) can only make RC4 files (revisions 2 and 3), so AES-128 and AES-256 from another producer stay untried here.

use crate::*;
use std::process::Command;

fn have(tool: &str, arg: &str) -> bool {
    Command::new(tool).arg(arg).output().is_ok()
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-pdf-gs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// PostScript: `lines` are (x, y, text) in points on an A4 page, set in Helvetica 12.
fn ps(lines: &[(i32, i32, &str)]) -> String {
    let mut s = String::from("%!PS\n/Helvetica findfont 12 scalefont setfont\n");
    for (x, y, t) in lines {
        s.push_str(&format!("{x} {y} moveto ({t}) show\n"));
    }
    s.push_str("showpage\n");
    s
}

/// `gs` turns the PostScript into a PDF; `extra` are its flags (the protection).
fn make(dir: &std::path::Path, name: &str, post: &str, extra: &[&str]) -> Option<Vec<u8>> {
    let src = dir.join(format!("{name}.ps"));
    let out = dir.join(format!("{name}.pdf"));
    std::fs::write(&src, post).unwrap();
    let o = Command::new("gs").env("TMPDIR", "/tmp").args(["-q", "-dNOPAUSE", "-dBATCH", "-dSAFER", "-sDEVICE=pdfwrite", "-dCompatibilityLevel=1.7"]).args(extra).arg(format!("-sOutputFile={}", out.display())).arg(&src).output().ok()?;
    if !o.status.success() {
        eprintln!("gs refused {extra:?}: {}", String::from_utf8_lossy(&o.stderr));
        return None;
    }
    std::fs::read(&out).ok()
}

fn pdftotext(dir: &std::path::Path, name: &str, bytes: &[u8], pw: Option<&str>, extra: &[&str]) -> Option<String> {
    let f = dir.join(format!("{name}-read.pdf"));
    std::fs::write(&f, bytes).unwrap();
    let mut c = Command::new("pdftotext");
    if let Some(p) = pw {
        c.args(["-upw", p]);
    }
    let o = c.args(extra).arg(&f).arg("-").output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn words(t: &str) -> Vec<String> {
    t.split_whitespace().map(|w| w.to_string()).collect()
}

const SECRET: &str = "Ghostscript made this secret page";

#[test]
fn files_made_by_ghostscript_with_a_password_open_here_with_the_same_words_pdftotext_reads() {
    if !have("gs", "--version") || !have("pdftotext", "-v") {
        eprintln!("skipped: needs gs and pdftotext");
        return;
    }
    let d = tmp("enc");
    let post = ps(&[(72, 700, SECRET), (72, 680, "second line of the page")]);
    // (revision, key length): this Ghostscript makes only revisions 2 and 3 (RC4); AES needs another producer (qpdf is not on the build host)
    for (r, bits, label) in [(2, 40, "RC4 40 bit R2"), (3, 40, "RC4 40 bit R3"), (3, 96, "RC4 96 bit R3"), (3, 128, "RC4 128 bit R3")] {
        let args = [format!("-dEncryptionR={r}"), format!("-dKeyLength={bits}"), "-sOwnerPassword=owner-pw".to_string(), "-sUserPassword=user-pw".to_string()];
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let Some(bytes) = make(&d, &format!("e{r}"), &post, &refs) else { panic!("{label}: gs did not make the file") };
        assert!(String::from_utf8_lossy(&bytes).contains("/Encrypt"), "{label}: gs really encrypted it");
        // the other reader needs the password too, and reads the words with it
        assert!(pdftotext(&d, "nopw", &bytes, None, &[]).is_none(), "{label}: pdftotext refuses without a password");
        let theirs = pdftotext(&d, "pw", &bytes, Some("user-pw"), &[]).unwrap_or_else(|| panic!("{label}: pdftotext with the user password"));
        assert!(theirs.contains(SECRET), "{label}: {theirs:?}");
        // ours: asks for the password, refuses a wrong one, opens with the user or the owner password and reads the same words
        assert_eq!(Document::open(bytes.clone()).err(), Some(PdfError::PasswordNeeded), "{label}: no password");
        assert_eq!(Document::open_with_password(bytes.clone(), "wrong").err(), Some(PdfError::WrongPassword), "{label}: wrong password");
        for pw in ["user-pw", "owner-pw"] {
            let doc = Document::open_with_password(bytes.clone(), pw).unwrap_or_else(|e| panic!("{label} with {pw}: {e}"));
            assert_eq!(doc.page_count(), 1);
            let ours = doc.page_text(0).unwrap();
            assert_eq!(words(&ours), words(&theirs), "{label} with {pw}: our words are the other reader's words");
        }
    }
}

#[test]
fn two_columns_made_by_ghostscript_are_read_in_the_order_pdftotext_reads_them() {
    if !have("gs", "--version") || !have("pdftotext", "-v") {
        eprintln!("skipped: needs gs and pdftotext");
        return;
    }
    let d = tmp("cols");
    // a heading across the page, then two columns whose lines interleave when read by height
    let l1 = ["left alpha one", "left beta two", "left gamma three", "left delta four", "left epsilon five", "left zeta six"];
    let r1 = ["right omega one", "right psi two", "right chi three", "right phi four", "right upsilon five", "right tau six"];
    let mut lines: Vec<(i32, i32, &str)> = vec![(72, 780, "A heading above both columns")];
    for i in 0..6 {
        lines.push((72, 740 - 16 * i as i32, l1[i]));
        lines.push((330, 740 - 16 * i as i32, r1[i]));
    }
    let Some(bytes) = make(&d, "cols", &ps(&lines), &[]) else { panic!("gs did not make the two-column file") };
    let theirs = pdftotext(&d, "cols", &bytes, None, &[]).expect("pdftotext reads it");
    let ours = Document::open(bytes).unwrap().page_text(0).unwrap();
    let (a, b) = (words(&ours), words(&theirs));
    assert_eq!(a, b, "ours:\n{ours}\ntheirs:\n{theirs}");
    // and the order is the columns one after the other, not line by line across
    let pos = |w: &str| a.iter().position(|x| x == w).unwrap();
    assert!(pos("six") < pos("omega") || pos("alpha") < pos("omega"), "the left column comes before the right");
    assert!(pos("left") < pos("right") && pos("zeta") < pos("omega"), "the whole left column is read before the right one: {a:?}");
}

/// Real documents from other producers: every PDF under the folders in `IYAKKI_PDF_CORPUS` (colon separated) is read here and by
/// `pdftotext`; the words pdftotext finds must also be found here (recall, counting each word as often as the other reader has it).
/// Prints one line per file. Skipped without the variable.
#[test]
fn real_documents_give_most_of_the_words_pdftotext_finds() {
    let Ok(dirs) = std::env::var("IYAKKI_PDF_CORPUS") else {
        eprintln!("skipped: IYAKKI_PDF_CORPUS is not set");
        return;
    };
    if !have("pdftotext", "-v") {
        eprintln!("skipped: needs pdftotext");
        return;
    }
    let mut files = Vec::new();
    for dir in dirs.split(':').filter(|d| !d.is_empty()) {
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(p) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    let q = e.path();
                    if q.is_dir() {
                        stack.push(q);
                    } else if q.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
                        files.push(q);
                    }
                }
            }
        }
    }
    files.sort();
    assert!(!files.is_empty(), "no PDF under {dirs}");
    let d = tmp("corpus");
    let mut worst: f64 = 1.0;
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let Some(theirs) = pdftotext(&d, "c", &bytes, None, &[]) else {
            eprintln!("{}: pdftotext cannot read it, skipped", f.display());
            continue;
        };
        let doc = match Document::open(bytes) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("{}: not opened here: {e}", f.display());
                worst = 0.0;
                continue;
            }
        };
        let mut ours = String::new();
        for i in 0..doc.page_count() {
            if let Ok(t) = doc.page_text(i) {
                ours.push_str(&t);
                ours.push('\n');
            }
        }
        let mut have_w: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for w in words(&ours) {
            *have_w.entry(w).or_insert(0) += 1;
        }
        let tw = words(&theirs);
        let mut found = 0usize;
        for w in &tw {
            if let Some(n) = have_w.get_mut(w) {
                if *n > 0 {
                    *n -= 1;
                    found += 1;
                }
            }
        }
        let recall = if tw.is_empty() { 1.0 } else { found as f64 / tw.len() as f64 };
        eprintln!("{}: {} pages, {} words by pdftotext, {} of them found here ({:.1} %)", f.file_name().unwrap().to_string_lossy(), doc.page_count(), tw.len(), found, recall * 100.0);
        worst = worst.min(recall);
    }
    assert!(worst >= 0.80, "the worst file found only {:.1} % of the other reader's words", worst * 100.0);
}
