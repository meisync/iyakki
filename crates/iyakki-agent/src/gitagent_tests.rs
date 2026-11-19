use super::*;
use std::sync::Mutex;

fn short_dir(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    Path::new(&base).join(format!("ga-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]))
}

fn ask(sock: &Path, msg: &[u8]) -> Vec<u8> {
    let mut s = UnixStream::connect(sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut f = Vec::new();
    put_u32(&mut f, msg.len() as u32);
    f.extend_from_slice(msg);
    s.write_all(&f).unwrap();
    let mut l = [0u8; 4];
    s.read_exact(&mut l).unwrap();
    let mut out = vec![0u8; u32::from_be_bytes(l) as usize];
    s.read_exact(&mut out).unwrap();
    out
}

fn agent_with(keys: Vec<AgentKey>, tokens: Vec<TokenEntry>) -> (GitAgent, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
    let signs = Arc::new(Mutex::new(Vec::new()));
    let toks = Arc::new(Mutex::new(Vec::new()));
    let (s2, t2) = (signs.clone(), toks.clone());
    let a = GitAgent::start(&short_dir("a"), keys, tokens, Arc::new(move |id, fp| s2.lock().unwrap().push(format!("{id} {fp}"))), Arc::new(move |h| t2.lock().unwrap().push(h.to_string()))).unwrap();
    (a, signs, toks)
}

#[test]
fn the_agent_lists_identities_and_signs_and_never_returns_a_private_key() {
    let k1 = KeyPair::generate("one").unwrap();
    let k2 = KeyPair::generate("two").unwrap();
    let (agent, signs, _) = agent_with(vec![AgentKey { id: "work".into(), pair: k1.clone() }, AgentKey { id: "home".into(), pair: k2.clone() }], vec![]);
    let sock = agent.ssh_sock();
    assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(agent.dir()).unwrap().permissions().mode() & 0o777, 0o700);
    // identities
    let r = ask(&sock, &[SSH_AGENTC_REQUEST_IDENTITIES]);
    assert_eq!(r[0], SSH_AGENT_IDENTITIES_ANSWER);
    let mut rd = Reader { b: &r[1..] };
    assert_eq!(rd.u32(), Some(2));
    assert_eq!(rd.string().unwrap(), k1.public_blob().as_slice());
    assert_eq!(rd.string().unwrap(), b"one");
    assert_eq!(rd.string().unwrap(), k2.public_blob().as_slice());
    // sign with the second key, and check the signature with the public key
    let mut req = vec![SSH_AGENTC_SIGN_REQUEST];
    put_str(&mut req, &k2.public_blob());
    put_str(&mut req, b"session data to sign");
    put_u32(&mut req, 0);
    let r = ask(&sock, &req);
    assert_eq!(r[0], SSH_AGENT_SIGN_RESPONSE);
    let mut rd = Reader { b: &r[1..] };
    let blob = rd.string().unwrap();
    let mut sb = Reader { b: blob };
    assert_eq!(sb.string().unwrap(), b"ssh-ed25519");
    let sig = ed25519_dalek::Signature::from_slice(sb.string().unwrap()).unwrap();
    use ed25519_dalek::Verifier;
    ed25519_dalek::VerifyingKey::from_bytes(&k2.public_bytes()).unwrap().verify(b"session data to sign", &sig).unwrap();
    assert_eq!(signs.lock().unwrap().as_slice(), [format!("home {}", k2.fingerprint())]);
    // an unknown key, an add-key request and nonsense are refused
    let mut req = vec![SSH_AGENTC_SIGN_REQUEST];
    put_str(&mut req, &KeyPair::generate("x").unwrap().public_blob());
    put_str(&mut req, b"d");
    put_u32(&mut req, 0);
    assert_eq!(ask(&sock, &req), vec![SSH_AGENT_FAILURE]);
    assert_eq!(ask(&sock, &[17, 1, 2, 3]), vec![SSH_AGENT_FAILURE]);
    assert_eq!(ask(&sock, &[SSH_AGENTC_SIGN_REQUEST, 0, 0]), vec![SSH_AGENT_FAILURE]);
    assert_eq!(signs.lock().unwrap().len(), 1);
    // the folder and the sockets are gone when the agent is dropped
    let dir = agent.dir().to_path_buf();
    drop(agent);
    assert!(!dir.exists());
}

#[test]
fn askpass_answers_only_for_hosts_with_a_token_and_reports_it() {
    let tokens = vec![TokenEntry { host: "git.example.org".into(), user: "alice".into(), token: Zeroizing::new("s3cret-token".into()) }];
    let (agent, _, toks) = agent_with(vec![], tokens);
    let sock = agent.askpass_sock();
    let q = |p: &str| askpass_client(sock.to_str().unwrap(), p).unwrap();
    assert_eq!(q("Username for 'https://git.example.org': "), "alice");
    assert_eq!(q("Password for 'https://alice@git.example.org': "), "s3cret-token");
    assert_eq!(q("Password for 'https://alice@git.example.org:443': "), "s3cret-token");
    assert_eq!(q("Password for 'https://bob@git.example.org': "), "", "another user name gets nothing");
    assert_eq!(q("Password for 'https://other.example.org': "), "", "another host gets nothing");
    assert_eq!(q("Passphrase for key: "), "");
    assert_eq!(toks.lock().unwrap().as_slice(), ["git.example.org", "git.example.org"]);
    // the link git is pointed at exists and carries the helper's name
    assert!(agent.askpass_program().symlink_metadata().is_ok());
    assert_eq!(agent.askpass_program().file_name().unwrap(), ASKPASS_NAME);
}

#[test]
fn prompts_are_read_into_host_and_user() {
    assert_eq!(prompt_host("Username for 'https://github.com': "), Some((true, "github.com".into(), None)));
    assert_eq!(prompt_host("Password for 'https://me@GitHub.com:8443/x': "), Some((false, "github.com".into(), Some("me".into()))));
    assert_eq!(prompt_host("nothing quoted"), None);
}

#[test]
fn a_path_that_is_too_long_for_a_socket_is_said_plainly() {
    let long = Path::new("/tmp").join("x".repeat(120));
    let e = GitAgent::start(&long, vec![], vec![], Arc::new(|_, _| {}), Arc::new(|_| {})).err().unwrap();
    assert!(e.contains("too deep"), "{e}");
}

#[test]
fn askpass_main_hook_only_acts_under_its_own_name() {
    assert_eq!(maybe_askpass("/usr/bin/iyakki", &["x".into()]), None);
    assert_eq!(maybe_askpass("/run/x/iyakki-git-askpass", &["Username for 'https://nowhere': ".into()]), Some(1), "no socket in the environment: fail, do not hang");
}

#[test]
fn the_real_openssh_tools_list_and_sign_through_the_agent() {
    use std::process::Command;
    if Command::new("ssh-add").arg("-?").output().is_err() || Command::new("ssh-keygen").arg("-?").output().is_err() {
        eprintln!("skipped: no OpenSSH tools");
        return;
    }
    let k = KeyPair::generate("agent@test").unwrap();
    let (agent, signs, _) = agent_with(vec![AgentKey { id: "t".into(), pair: k.clone() }], vec![]);
    // ssh-add -L prints the public key the agent holds
    let out = Command::new("ssh-add").arg("-L").env("SSH_AUTH_SOCK", agent.ssh_sock()).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success() && text.trim() == k.public_line(), "{text} / {}", String::from_utf8_lossy(&out.stderr));
    // ssh-keygen -Y sign -U asks the agent to sign; the signature then verifies against the public key alone
    let d = short_dir("real");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("key.pub"), format!("{}\n", k.public_line())).unwrap();
    std::fs::write(d.join("msg"), "hello agent").unwrap();
    let s = Command::new("ssh-keygen").args(["-Y", "sign", "-n", "file", "-U", "-f"]).arg(d.join("key.pub")).arg(d.join("msg")).env("SSH_AUTH_SOCK", agent.ssh_sock()).output().unwrap();
    assert!(s.status.success(), "{}", String::from_utf8_lossy(&s.stderr));
    assert_eq!(signs.lock().unwrap().len(), 1);
    std::fs::write(d.join("allowed"), format!("t {}\n", k.public_line())).unwrap();
    let v = Command::new("ssh-keygen").args(["-Y", "verify", "-n", "file", "-I", "t", "-f"]).arg(d.join("allowed")).arg("-s").arg(d.join("msg.sig")).stdin(std::fs::File::open(d.join("msg")).unwrap()).output().unwrap();
    assert!(v.status.success(), "{}{}", String::from_utf8_lossy(&v.stdout), String::from_utf8_lossy(&v.stderr));
    let _ = std::fs::remove_dir_all(&d);
}
