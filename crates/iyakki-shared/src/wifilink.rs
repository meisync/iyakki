//! The live line of Settings -> Network about the joined WiFi network (F-NET-07): the signal read every few seconds from `iw dev X
//! link` and, only when the person chose a check address, the sign-in (captive portal) check. The timing is
//! `iyakki_devctl::wifi_ext::LinkTimers`; the work runs on a short-lived thread and the page only reads the last answer, so a slow
//! adapter or network never stops the page. Nothing is sent anywhere unless `wifi-probe.txt` in the data folder names a check address.
// SPDX-License-Identifier: Apache-2.0

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_devctl::wifi_ext::{parse_link, portal_sentence, Link, LinkTimers, PortalState, SignalTracker};

pub type LinkJob = Arc<dyn Fn() -> Option<String> + Send + Sync>;
pub type ProbeJob = Arc<dyn Fn() -> PortalState + Send + Sync>;

#[derive(Default)]
pub struct LinkWatch {
    timers: LinkTimers,
    tracker: SignalTracker,
    portal: Option<PortalState>,
    ssid: Option<String>,
    sig_rx: Option<Receiver<Option<Link>>>,
    probe_rx: Option<Receiver<PortalState>>,
    /// Not before this second: a read is started at most every five seconds, joined or not.
    next_read: u64,
}

impl LinkWatch {
    /// Collect finished answers, then start what is due. `now` counts seconds from any steady start.
    pub fn tick(&mut self, now: u64, link: Option<LinkJob>, probe: Option<ProbeJob>) {
        if let Some(rx) = &self.sig_rx {
            if let Ok(l) = rx.try_recv() {
                self.sig_rx = None;
                self.timers.signal_done(now);
                match l {
                    Some(l) => {
                        if self.ssid.as_deref() != Some(l.ssid.as_str()) {
                            self.tracker = SignalTracker::new();
                            self.portal = None;
                            self.timers.joined(now);
                            self.ssid = Some(l.ssid.clone());
                        }
                        self.tracker.push(l.signal_dbm);
                    }
                    None => {
                        self.timers.left();
                        self.tracker = SignalTracker::new();
                        self.portal = None;
                        self.ssid = None;
                    }
                }
            }
        }
        if let Some(rx) = &self.probe_rx {
            if let Ok(p) = rx.try_recv() {
                self.probe_rx = None;
                self.timers.probe_done(now, &p);
                self.portal = Some(p);
            }
        }
        if self.sig_rx.is_none() && link.is_some() && now >= self.next_read && (self.ssid.is_none() || self.timers.signal_due(now)) {
            self.next_read = now + LinkTimers::SIGNAL_EVERY;
            let (tx, rx) = channel();
            let job = link.unwrap_or_else(|| Arc::new(|| None));
            std::thread::spawn(move || {
                let _ = tx.send(job().and_then(|t| parse_link(&t)));
            });
            self.sig_rx = Some(rx);
        }
        if self.probe_rx.is_none() && self.timers.probe_due(now) {
            if let Some(job) = probe {
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let _ = tx.send(job());
                });
                self.probe_rx = Some(rx);
            }
        }
    }

    /// The sentences for the page, or nothing when no network is joined.
    pub fn line(&self) -> Option<String> {
        let ssid = self.ssid.as_ref()?;
        let mut s = format!("{ssid}. {}", self.tracker.sentence());
        if let Some(p) = &self.portal {
            s.push(' ');
            s.push_str(&portal_sentence(p));
        }
        Some(s)
    }

    /// The sign-in address to open, when the check found a page asking for one.
    pub fn sign_in_url(&self) -> Option<String> {
        match &self.portal {
            Some(PortalState::SignIn(u)) => u.clone(),
            _ => None,
        }
    }
}

/// The check as a job, when the person chose an address.
pub fn probe_job(dir: &std::path::Path) -> Option<ProbeJob> {
    probe_setting(dir).map(|(addr, host, path, kind)| -> ProbeJob { Arc::new(move || iyakki_devctl::wifi_ext::probe_once(&addr, &host, &path, kind, std::time::Duration::from_secs(4))) })
}

/// The check address the person chose (`host` or `host:port`, then a path), from `wifi-probe.txt` in the data folder: one line
/// `HOST[:PORT] /PATH [204|success]`. No file means no check.
pub fn probe_setting(dir: &std::path::Path) -> Option<(String, String, String, iyakki_devctl::wifi_ext::Probe)> {
    let t = std::fs::read_to_string(dir.join("wifi-probe.txt")).ok()?;
    let mut w = t.lines().find(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))?.split_whitespace();
    let hp = w.next()?.to_string();
    let path = w.next().unwrap_or("/").to_string();
    let kind = if w.next() == Some("success") { iyakki_devctl::wifi_ext::Probe::SuccessPage } else { iyakki_devctl::wifi_ext::Probe::NoContent };
    let host = hp.split(':').next()?.to_string();
    let addr = if hp.contains(':') { hp } else { format!("{hp}:80") };
    iyakki_devctl::wifi_ext::probe_request(&host, &path).ok()?;
    Some((addr, host, path, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "Connected to aa:bb:cc:00:00:01 (on wlan0)\n\tSSID: Seen\n\tfreq: 2437.0\n\tsignal: -52 dBm\n\ttx bitrate: 130.0 MBit/s\n";

    fn settle(w: &mut LinkWatch, now: u64, link: &Option<LinkJob>, probe: &Option<ProbeJob>) {
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            w.tick(now, link.clone(), probe.clone());
            if w.sig_rx.is_none() && w.probe_rx.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the background reads did not finish");
    }

    #[test]
    fn the_signal_is_read_on_its_timer_and_the_sign_in_check_runs_only_when_an_address_is_chosen() {
        let (reads, probes) = (Arc::new(std::sync::atomic::AtomicU32::new(0)), Arc::new(std::sync::atomic::AtomicU32::new(0)));
        let r2 = reads.clone();
        let link: Option<LinkJob> = Some(Arc::new(move || {
            r2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(LINK.to_string())
        }));
        let p2 = probes.clone();
        let probe: Option<ProbeJob> = Some(Arc::new(move || {
            p2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            PortalState::SignIn(Some("http://portal.example/login".into()))
        }));
        let mut w = LinkWatch::default();
        assert!(w.line().is_none());
        settle(&mut w, 100, &link, &None);
        assert!(w.line().unwrap().starts_with("Seen. Signal: excellent (-52 dBm"), "{:?}", w.line());
        // nothing is due one second later, a read is due five seconds later, the check is never run without an address
        settle(&mut w, 101, &link, &None);
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        settle(&mut w, 105, &link, &None);
        assert_eq!((reads.load(std::sync::atomic::Ordering::SeqCst), probes.load(std::sync::atomic::Ordering::SeqCst)), (2, 0));
        // with an address: first check three seconds after the join, then every ten seconds while a sign-in is wanted
        settle(&mut w, 110, &link, &probe);
        assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(w.sign_in_url().as_deref(), Some("http://portal.example/login"));
        assert!(w.line().unwrap().contains("wants a sign-in"));
        settle(&mut w, 113, &link, &probe);
        assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
        settle(&mut w, 121, &link, &probe);
        assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 2);
        // the network is left: the line goes away
        let gone: Option<LinkJob> = Some(Arc::new(|| Some("Not connected.\n".to_string())));
        settle(&mut w, 130, &gone, &probe);
        assert!(w.line().is_none() && w.sign_in_url().is_none());
    }

    #[test]
    fn the_check_address_comes_only_from_the_file_and_must_be_a_plain_host() {
        let d = std::env::temp_dir().join(format!("iyakki-probe-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(probe_setting(&d).is_none(), "no file, no check");
        std::fs::write(d.join("wifi-probe.txt"), "# the address\ncheck.example.net /generate_204\n").unwrap();
        let (addr, host, path, kind) = probe_setting(&d).unwrap();
        assert_eq!((addr.as_str(), host.as_str(), path.as_str(), kind), ("check.example.net:80", "check.example.net", "/generate_204", iyakki_devctl::wifi_ext::Probe::NoContent));
        std::fs::write(d.join("wifi-probe.txt"), "10.0.0.1:8080 /ok success\n").unwrap();
        assert_eq!(probe_setting(&d).unwrap().3, iyakki_devctl::wifi_ext::Probe::SuccessPage);
        std::fs::write(d.join("wifi-probe.txt"), "bad_host!/x /generate_204\n").unwrap();
        assert!(probe_setting(&d).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
