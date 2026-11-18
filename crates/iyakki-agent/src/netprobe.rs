//! Small HTTP calls the tools make on their own (is the registry answering, list its images, delete a manifest) through the
//! in-house client ([`crate::gitnet::NetGit`]): the image ships no `curl`. No token is involved. In tests the network is never
//! touched unless a test asks for it: answers are scripted per thread with [`fake`].

use crate::gitconn::{GitHttp, NetErr, Req, Style};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Answer {
    pub fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
    }
    pub fn success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// One call. `insecure`: any certificate is accepted (reachability only). Errors are one readable line.
pub fn call(method: &str, url: &str, accept: Option<&str>, timeout_s: u64, max_bytes: usize, insecure: bool) -> Result<Answer, String> {
    #[cfg(test)]
    if let Some(r) = fake_lookup(method, url) {
        return r;
    }
    let net = crate::gitnet::NetGit { ca_files: Vec::new(), insecure };
    let r = Req { method: method.into(), url: url.into(), token: None, style: Style::Token, body: None, cert: None, accept: accept.map(str::to_string), max_bytes, timeout_s };
    match net.send(&r) {
        Ok(o) => Ok(Answer { status: o.status, headers: o.headers, body: o.body }),
        Err(NetErr::Unreachable(m)) => Err(if m.trim().is_empty() { "could not connect".into() } else { m.lines().next().unwrap_or("").trim().to_string() }),
        Err(NetErr::Tls(m)) | Err(NetErr::Other(m)) => Err(m.lines().next().unwrap_or("").trim().to_string()),
    }
}

/// `GET url` is answered with a 2xx.
pub fn get_ok(url: &str, timeout_s: u64) -> bool {
    call("GET", url, None, timeout_s, 4096, false).map(|a| a.success()).unwrap_or(false)
}

/// `GET url` answered 2xx: the body; otherwise one line saying why (like `curl -f`).
pub fn get_body(url: &str, timeout_s: u64, max_bytes: usize) -> Result<String, String> {
    let a = call("GET", url, None, timeout_s, max_bytes, false)?;
    if a.success() {
        Ok(a.body)
    } else {
        Err(format!("the server answered HTTP {}", a.status))
    }
}

#[cfg(test)]
type Rules = Vec<(String, Result<Answer, String>)>;

#[cfg(test)]
thread_local! {
    /// None: really use the network (a test with a local listener); Some: scripted, an unmatched call is "could not connect".
    static FAKE: std::cell::RefCell<Option<Rules>> = const { std::cell::RefCell::new(Some(Vec::new())) };
}

#[cfg(test)]
fn fake_lookup(method: &str, url: &str) -> Option<Result<Answer, String>> {
    let line = format!("{method} {url}");
    FAKE.with(|f| f.borrow().as_ref().map(|rules| rules.iter().find(|(k, _)| line.contains(k.as_str())).map(|(_, r)| r.clone()).unwrap_or_else(|| Err("could not connect".into()))))
}

/// Tests: script an answer for the calls whose `"METHOD url"` contains `rule` (the first rule that matches wins).
#[cfg(test)]
pub fn fake(rule: &str, status: u16, headers: &[(&str, &str)], body: &str) {
    let a = Answer { status, headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body: body.into() };
    FAKE.with(|f| f.borrow_mut().get_or_insert_with(Vec::new).push((rule.into(), Ok(a))));
}

/// Tests: a call that fails to connect.
#[cfg(test)]
pub fn fake_down(rule: &str, why: &str) {
    FAKE.with(|f| f.borrow_mut().get_or_insert_with(Vec::new).push((rule.into(), Err(why.into()))));
}

/// Tests: forget the scripted answers of this thread.
#[cfg(test)]
pub fn fake_reset() {
    FAKE.with(|f| *f.borrow_mut() = Some(Vec::new()));
}

/// Tests: use the real network (local listeners) on this thread.
#[cfg(test)]
pub fn real_network() {
    FAKE.with(|f| *f.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_local_listener_is_asked_with_get_and_a_closed_port_is_one_line() {
        real_network();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            for code in ["200 OK", "404 Not Found"] {
                let (mut s, _) = l.accept().unwrap();
                let mut b = vec![0u8; 4096];
                let _ = s.read(&mut b).unwrap();
                let _ = s.write_all(format!("HTTP/1.1 {code}\r\nContent-Length: 2\r\nDocker-Content-Digest: sha256:ab\r\n\r\n{{}}").as_bytes());
            }
        });
        let base = format!("http://127.0.0.1:{port}");
        let a = call("GET", &format!("{base}/v2/"), Some("application/json"), 5, 4096, false).unwrap();
        assert!(a.success() && a.body == "{}" && a.header("docker-content-digest") == Some("sha256:ab"));
        assert_eq!(get_body(&format!("{base}/v2/x"), 5, 4096).unwrap_err(), "the server answered HTTP 404");
        t.join().unwrap();
        assert!(!get_ok(&format!("{base}/v2/"), 2));
    }

    #[test]
    fn scripted_answers_never_touch_the_network() {
        fake("GET http://reg:1/v2/", 200, &[], "{}");
        assert!(get_ok("http://reg:1/v2/", 1));
        assert!(!get_ok("http://other:1/v2/", 1));
    }
}
