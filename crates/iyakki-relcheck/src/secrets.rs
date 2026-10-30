//! A scan of the tree for things that must not be published: private keys, cloud and API tokens, `.env` files, ssh keys, the
//! owner's own key names. Findings are `Block` (a real-looking secret: the release fails) or `Review` (a test fixture, a
//! pattern in source code that detects secrets, a password in a test: listed, the owner looks).

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Level {
    Block,
    Review,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub file: PathBuf,
    pub line: usize,
    pub what: String,
}

const SKIP_DIRS: [&str; 6] = ["target", ".git", "node_modules", ".cargo", "legal-info", "buildroot-src"];
const SECRET_FILE_NAMES: [&str; 8] = [".env", "id_rsa", "id_ed25519", "id_ecdsa", "authorized_keys", "iyakki_diag", "release.key", ".netrc"];
const SECRET_EXTENSIONS: [&str; 4] = ["pem", "key", "p12", "pfx"];

fn is_base64_line(l: &str) -> bool {
    let l = l.trim();
    l.len() >= 40 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

fn is_test_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.contains("/tests/") || s.contains("fixtures") || s.contains("/examples/") || s.ends_with("_tests.rs") || s.contains("/samples/")
}

fn token_after<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let i = line.find(prefix)?;
    Some(&line[i + prefix.len()..])
}

/// Keyboard-walk and counting bodies are what test fixtures use ("abcdefghij...", "1234567890").
fn fake_body(s: &str) -> bool {
    let b: String = s.chars().take(40).collect::<String>().to_lowercase();
    ["abcdefghij", "1234567890", "0123456789", "xxxxxxxx", "aaaaaaaa", "fake", "example", "dummy"].iter().any(|k| b.contains(k))
}

fn all_of(s: &str, n: usize, ok: impl Fn(char) -> bool) -> bool {
    s.chars().take_while(|c| ok(*c)).count() >= n
}

/// Private key-file names to block, from `IYAKKI_PRIVATE_KEY_NAMES` (comma-separated, e.g. `adminKey,iyakkiKey`). Kept out of
/// the tree so the checker never publishes the names it guards.
fn private_key_names() -> Vec<String> {
    std::env::var("IYAKKI_PRIVATE_KEY_NAMES")
        .map(|v| v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// Scan the text of one file (`path` decides how strict the verdict is).
pub fn scan_text(path: &Path, text: &str) -> Vec<Finding> {
    scan_text_with(path, text, &private_key_names())
}

/// [`scan_text`] with an explicit list of private key-file names (lower case).
pub fn scan_text_with(path: &Path, text: &str, private_names: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut f = |level: Level, line: usize, what: &str| out.push(Finding { level, file: path.to_path_buf(), line, what: what.to_string() });
    let lines: Vec<&str> = text.lines().collect();
    let source_or_doc = path.extension().is_some_and(|e| e == "rs" || e == "md" || e == "py" || e == "sh");
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY-----") {
            let real = lines.get(i + 1).is_some_and(|l| is_base64_line(l));
            if real {
                f(Level::Block, n, "a private key block with a key body");
            } else {
                f(Level::Review, n, "a private key header (no key body: a detector or a doc?)");
            }
        }
        if token_after(line, "AKIA").is_some_and(|r| all_of(r, 16, |c| c.is_ascii_uppercase() || c.is_ascii_digit())) {
            f(if is_test_path(path) || source_or_doc { Level::Review } else { Level::Block }, n, "looks like an AWS access key id");
        }
        for (prefix, len, what) in [("ghp_", 36, "a GitHub token"), ("github_pat_", 30, "a GitHub token"), ("xoxb-", 20, "a Slack token"), ("xoxp-", 20, "a Slack token"), ("sk-ant-", 20, "an Anthropic API key"), ("AIza", 35, "a Google API key")] {
            if let Some(r) = token_after(line, prefix).filter(|r| all_of(r, len.min(20), |c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && (len <= 20 || all_of(r, len, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || fake_body(r))) {
                f(if is_test_path(path) || fake_body(r) { Level::Review } else { Level::Block }, n, &format!("looks like {what}"));
            }
        }
        if token_after(line, "sk-").is_some_and(|r| all_of(r, 40, |c| c.is_ascii_alphanumeric())) && !line.contains("sk-ant-") {
            f(if is_test_path(path) { Level::Review } else { Level::Block }, n, "looks like an OpenAI API key");
        }
        if private_names.iter().any(|k| line.to_lowercase().contains(k.as_str())) {
            f(Level::Block, n, "a private key file name (IYAKKI_PRIVATE_KEY_NAMES)");
        }
        let low = line.to_lowercase();
        for key in ["password", "passwd", "secret", "api_key", "apikey", "token"] {
            for sep in ["=\"", ": \"", "= \"", " = \"", "='", "= '", " = '", ": '"] {
                if let Some(i) = low.find(&format!("{key}{sep}")) {
                    let rest = &line[i + key.len() + sep.len()..];
                    let value: String = rest.chars().take_while(|c| *c != '"' && *c != '\'').collect();
                    let plausible = value.len() >= 6 && !value.contains(' ') && !value.contains('{') && !value.contains('$') && !value.contains("..") && !value.to_lowercase().contains("example") && !value.contains('<');
                    if plausible {
                        f(Level::Review, n, &format!("a literal value after `{key}`"));
                    }
                }
            }
        }
    }
    out
}

/// Is this file name one that usually holds a secret?
pub fn secret_file_name(name: &str) -> bool {
    SECRET_FILE_NAMES.contains(&name) || Path::new(name).extension().is_some_and(|e| SECRET_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<Finding>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&p, root, out);
            continue;
        }
        if secret_file_name(&name) {
            out.push(Finding { level: if is_test_path(&rel) { Level::Review } else { Level::Block }, file: rel.clone(), line: 0, what: "a file name that usually holds a secret (key, .env, ssh key)".into() });
        }
        let Ok(meta) = e.metadata() else { continue };
        if meta.len() > 4_000_000 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        for mut fd in scan_text(&rel, &text) {
            fd.file = rel.clone();
            out.push(fd);
        }
    }
}

/// Scan a repository folder.
pub fn scan_tree(root: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lv(path: &str, text: &str) -> Vec<(Level, String)> {
        scan_text(Path::new(path), text).into_iter().map(|f| (f.level, f.what)).collect()
    }

    #[test]
    fn a_real_private_key_blocks_and_a_detector_string_does_not() {
        let body = "A".repeat(64);
        let r = lv("keys/x.txt", &format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{body}\n-----END OPENSSH PRIVATE KEY-----\n"));
        assert_eq!(r[0].0, Level::Block);
        let r = lv("crates/a/src/guard.rs", "let h = \"-----BEGIN PRIVATE KEY-----\";\n");
        assert_eq!(r[0].0, Level::Review);
    }

    #[test]
    fn tokens_block_outside_tests_and_are_reviewed_inside() {
        let t = format!("let t = \"ghp_{}\";", "a1".repeat(18));
        assert_eq!(lv("crates/a/src/x.rs", &t)[0].0, Level::Block);
        assert_eq!(lv("crates/a/tests/x.rs", &t)[0].0, Level::Review);
        assert!(lv("README.md", "ghp_ is a prefix").is_empty());
        assert_eq!(lv("a.toml", "key = \"AKIAABCDEFGHIJKLMNOP\"")[0].0, Level::Block);
    }

    #[test]
    fn the_owners_key_name_and_literal_passwords_are_found() {
        let names = vec!["adminkey".to_string()];
        assert_eq!(scan_text_with(Path::new("x.sh"), "ssh -i ~/.ssh/adminKey host", &names)[0].level, Level::Block);
        assert!(scan_text_with(Path::new("x.sh"), "ssh -i ~/.ssh/adminKey host", &[]).is_empty());
        let r = lv("crates/a/src/t.rs", "let password = \"hunter2hunter2\";");
        assert_eq!(r, vec![(Level::Review, "a literal value after `password`".to_string())]);
        assert!(lv("a.rs", "password = \"\"; token: \"{{x}}\"; secret=\"$SECRET\"").is_empty());
    }

    #[test]
    fn secret_file_names() {
        assert!(secret_file_name(".env") && secret_file_name("server.pem") && secret_file_name("release.key") && secret_file_name("id_ed25519"));
        assert!(!secret_file_name("main.rs") && !secret_file_name("keys.md"));
    }

    #[test]
    fn a_tree_is_walked_and_target_is_skipped() {
        let d = std::env::temp_dir().join(format!("iyakki-relcheck-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("target")).unwrap();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join(".env"), "A=1").unwrap();
        std::fs::write(d.join("target/leak.pem"), "x").unwrap();
        std::fs::write(d.join("src/a.rs"), "fn main() {}").unwrap();
        let f = scan_tree(&d);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].file, PathBuf::from(".env"));
        assert_eq!(f[0].level, Level::Block);
        let _ = std::fs::remove_dir_all(&d);
    }
}
