//! A minimal SMTP client for the email channel. Rules, all tested against a scripted server:
//! - TLS or nothing: implicit TLS (port 465) or STARTTLS (587); plain SMTP is not a mode. Credentials and the message are only sent on
//!   a connection that [`MailStream::is_tls`] confirms, and the capabilities are asked again after STARTTLS.
//! - one message, one recipient; the From address is fixed by the channel, never by the notification.
//! - header injection is refused, not cleaned: a CR, LF or NUL in an address or the subject stops the send before any byte goes out.
//! - text/plain UTF-8 only, sent as base64 (no 8-bit surprises, no dot-stuffing needed).
//! The transport (`MailStream`) is a trait: tests have a fake server; the real one is in `transport.rs`.

use crate::wsproto::base64;

pub trait MailStream {
    /// One line of the server, without the line end.
    fn read_line(&mut self) -> Result<String, String>;
    fn write_all(&mut self, b: &[u8]) -> Result<(), String>;
    /// Upgrade this connection to TLS (after the server said 220 to STARTTLS).
    fn start_tls(&mut self) -> Result<(), String>;
    fn is_tls(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    Implicit,
    Starttls,
}

pub struct Auth<'a> {
    pub user: &'a str,
    pub pass: &'a str,
}

pub struct Mail<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
    pub ehlo_name: &'a str,
    pub now: u64,
    pub id: &'a str,
}

/// A plain address: one `@`, ASCII, none of the characters that open a header or an address list.
pub fn valid_address(a: &str) -> bool {
    let Some((l, d)) = a.split_once('@') else { return false };
    !l.is_empty() && l.len() <= 64 && !d.is_empty() && d.len() <= 255 && !d.contains('@') && d.contains('.')
        && l.chars().all(|c| c.is_ascii_alphanumeric() || "._%+-'".contains(c))
        && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !d.starts_with(['.', '-']) && !d.ends_with(['.', '-']) && !d.contains("..")
}

fn header_safe(s: &str) -> bool {
    !s.chars().any(|c| c == '\r' || c == '\n' || c == '\0' || (c.is_control() && c != '\t'))
}

fn encoded_subject(s: &str) -> String {
    if s.is_ascii() { s.to_string() } else { format!("=?UTF-8?B?{}?=", base64(s.as_bytes())) }
}

fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn rfc_date(now: u64) -> String {
    let (days, s) = ((now / 86_400) as i64, now % 86_400);
    let (y, m, d) = civil(days);
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days.rem_euclid(7)) as usize];
    let mon = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m - 1) as usize];
    format!("{wd}, {d:02} {mon} {y} {:02}:{:02}:{:02} +0000", s / 3600, s % 3600 / 60, s % 60)
}

/// The message as it goes after `DATA`, ending with the lone dot.
pub fn message(m: &Mail) -> Result<String, String> {
    if !valid_address(m.from) || !valid_address(m.to) {
        return Err("refused: an address is not a plain address".into());
    }
    if !header_safe(m.subject) || !header_safe(m.id) {
        return Err("refused: a header value holds a line break or control character".into());
    }
    let domain = m.from.split_once('@').map_or("localhost", |x| x.1);
    let b = base64(m.body.replace("\r\n", "\n").as_bytes());
    let mut lines = String::new();
    for c in b.as_bytes().chunks(76) {
        lines.push_str(std::str::from_utf8(c).unwrap_or(""));
        lines.push_str("\r\n");
    }
    let id: String = m.id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    Ok(format!(
        "From: <{}>\r\nTo: <{}>\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: <{}.{}@{}>\r\nMIME-Version: 1.0\r\nAuto-Submitted: auto-generated\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}.\r\n",
        m.from, m.to, encoded_subject(m.subject), rfc_date(m.now), id, m.now, domain, lines
    ))
}

fn reply(s: &mut dyn MailStream) -> Result<(u16, Vec<String>), String> {
    let mut text = Vec::new();
    for _ in 0..100 {
        let l = s.read_line()?;
        if l.len() < 3 || l.len() > 1024 {
            return Err("the mail server's answer is not SMTP".into());
        }
        let code: u16 = l[..3].parse().map_err(|_| "the mail server's answer is not SMTP".to_string())?;
        text.push(l.get(4..).unwrap_or("").to_string());
        if l.as_bytes().get(3) != Some(&b'-') {
            return Ok((code, text));
        }
    }
    Err("the mail server's answer is too long".into())
}

fn send_line(s: &mut dyn MailStream, line: &str) -> Result<(), String> {
    if line.contains(['\r', '\n']) {
        return Err("refused: a command holds a line break".into());
    }
    s.write_all(format!("{line}\r\n").as_bytes())
}

fn expect(s: &mut dyn MailStream, line: Option<&str>, ok: &[u16], what: &str) -> Result<Vec<String>, String> {
    if let Some(l) = line {
        send_line(s, l)?;
    }
    let (c, t) = reply(s)?;
    if ok.contains(&c) { Ok(t) } else { Err(format!("the mail server refused {what} ({c})")) }
}

fn ehlo(s: &mut dyn MailStream, name: &str) -> Result<Vec<String>, String> {
    expect(s, Some(&format!("EHLO {name}")), &[250], "EHLO").map(|v| v.into_iter().map(|x| x.to_ascii_uppercase()).collect())
}

pub fn send(s: &mut dyn MailStream, mode: TlsMode, auth: Option<&Auth>, m: &Mail) -> Result<(), String> {
    // nothing is sent on a message that cannot be built
    let data = message(m)?;
    if !header_safe(m.ehlo_name) || m.ehlo_name.contains(' ') {
        return Err("refused: bad name for EHLO".into());
    }
    if mode == TlsMode::Implicit && !s.is_tls() {
        return Err("refused: implicit TLS was asked for and the connection is not encrypted".into());
    }
    expect(s, None, &[220], "the connection")?;
    let mut caps = ehlo(s, m.ehlo_name)?;
    if mode == TlsMode::Starttls {
        if !caps.iter().any(|c| c == "STARTTLS") {
            return Err("refused: the server does not offer STARTTLS; plain SMTP is never used".into());
        }
        expect(s, Some("STARTTLS"), &[220], "STARTTLS")?;
        s.start_tls()?;
        caps = ehlo(s, m.ehlo_name)?;
    }
    if !s.is_tls() {
        return Err("refused: the connection is not encrypted".into());
    }
    if let Some(a) = auth {
        if !header_safe(a.user) || !header_safe(a.pass) {
            return Err("refused: the login holds a control character".into());
        }
        let mechs = caps.iter().find(|c| c.starts_with("AUTH ") || c.starts_with("AUTH=")).cloned().unwrap_or_default();
        if mechs.contains("PLAIN") {
            let cred = base64(format!("\0{}\0{}", a.user, a.pass).as_bytes());
            expect(s, Some(&format!("AUTH PLAIN {cred}")), &[235], "the login")?;
        } else if mechs.contains("LOGIN") {
            expect(s, Some("AUTH LOGIN"), &[334], "the login")?;
            expect(s, Some(&base64(a.user.as_bytes())), &[334], "the login")?;
            expect(s, Some(&base64(a.pass.as_bytes())), &[235], "the login")?;
        } else {
            return Err("the server offers no login method this client uses (PLAIN, LOGIN)".into());
        }
    }
    expect(s, Some(&format!("MAIL FROM:<{}>", m.from)), &[250], "the sender")?;
    expect(s, Some(&format!("RCPT TO:<{}>", m.to)), &[250, 251], "the recipient")?;
    expect(s, Some("DATA"), &[354], "DATA")?;
    s.write_all(data.as_bytes())?;
    expect(s, None, &[250], "the message")?;
    let _ = send_line(s, "QUIT");
    Ok(())
}
