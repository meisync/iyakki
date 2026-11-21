//! File transfer of a client session (F-CLIENT-01; docs/development/CLIENT-SPLIT-DESIGN.md section 7d). EXPERIMENTAL, additive: the capability
//! `files.transfer`, the messages `file_list`, `file_get`, `file_put`, `file_end` and `file_cancel`, and the binary streams 6 (to the client) and 7.
//!
//! Everything goes through the core's files facet (the screen's `files_*` calls): the home folder only, no links, no hidden names, 8 MB at most, an
//! upload saved through the facet's change book. The person is asked on the machine's screen: one card for browsing the folder (once a session)
//! and one card for every file that goes either way; the broker (capability `files.client`, a hard ask the device must have been given) writes the
//! access log. Nothing is sent or saved before the card is answered yes.

use super::media::Ctx;
use super::*;
use iyakki_core_api::client::{self as proto, FileMsg, FileRow};

/// Chunks sent to the client in one turn of the session loop (so a big download never starves the rest of the session).
const CHUNKS_PER_TURN: usize = 4;
/// Entries of one listing sent to the page (a listing is held to an ordinary message).
const MAX_ROWS: usize = 300;

struct Upload {
    id: u32,
    path: String,
    size: usize,
    buf: Vec<u8>,
}

struct Download {
    id: u32,
    data: Vec<u8>,
    at: usize,
}

#[derive(Default)]
pub(super) struct FileState {
    up: Option<Upload>,
    down: Option<Download>,
    /// The card for browsing the folder was answered yes in this session.
    browsing: bool,
}

fn say_file(out: &mut Vec<u8>, m: &FileMsg) {
    out.extend(ws::text_frame(&proto::encode_file(m)));
}

impl Session<'_> {
    fn file_err(&mut self, id: u32, code: &str, message: &str) {
        say_file(&mut self.out, &FileMsg::FileErr { id, code: code.into(), message: message.into() });
    }

    /// Like `media_ctx`/`media_decide`, with the refusal said as a file message. The context is the same; the broker asks the card.
    fn file_gate(&mut self, id: u32, label: &str) -> Option<(String, Device)> {
        let Some(host) = self.ep.media.clone() else {
            self.file_err(id, "unsupported", "this machine does not serve files to clients");
            return None;
        };
        let (Some(person), Some(dev), Some((session, _))) = (self.person.clone(), self.device.clone(), self.open.as_ref()) else {
            self.complain(ErrorCode::NotSignedIn, "sign in first");
            return None;
        };
        if !self.agreed.iter().any(|c| c == proto::cap::FILE_TRANSFER) {
            self.file_err(id, "capability_missing", "this connection did not agree on files.transfer: offer it in hello");
            return None;
        }
        let c = Ctx { host, session: *session, person: person.clone(), dev: dev.clone() };
        if !self.ep.calls.role_allows(&person, "files.client") {
            (self.ep.audit)(&person, "client.call.refused", &format!("files.client role {}", dev.id));
            self.file_err(id, ErrorCode::RoleDenied.as_str(), "your role does not allow that");
            return None;
        }
        match self.decide_cap(&c, "files.client", None, label) {
            Ok(()) => Some((person, dev)),
            Err(d) => {
                self.file_err(id, d.code(), &d.to_string());
                None
            }
        }
    }

    pub(super) fn file_list(&mut self, dir: &str) {
        if self.files.browsing {
            // already answered yes in this session: the broker still checks the grant (a revoke is seen at once) but no new card is made
            if !self.agreed.iter().any(|c| c == proto::cap::FILE_TRANSFER) || self.person.is_none() {
                return self.file_err(0, "capability_missing", "this connection did not agree on files.transfer");
            }
        } else if self.file_gate(0, "list your home folder").is_none() {
            return;
        }
        self.files.browsing = true;
        let Some(screen) = self.screen.as_mut() else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
        match screen.files_list(dir) {
            Ok(l) => {
                let truncated = l.truncated || l.entries.len() > MAX_ROWS;
                let entries: Vec<FileRow> = l.entries.into_iter().take(MAX_ROWS).map(|e| FileRow { name: e.name, is_dir: e.is_dir, size: e.size }).collect();
                say_file(&mut self.out, &FileMsg::FileList { dir: l.dir, entries, truncated });
            }
            Err(e) => self.file_err(0, &e.code, &e.message),
        }
    }

    pub(super) fn file_get(&mut self, id: u32, path: &str) {
        if self.files.down.is_some() {
            return self.file_err(id, "busy", "one download at a time");
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let Some((person, dev)) = self.file_gate(id, &format!("send {name} to the device")) else { return };
        let Some(screen) = self.screen.as_mut() else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
        match screen.files_read(path) {
            Ok(data) if data.len() <= proto::MAX_FILE => {
                (self.ep.audit)(&person, "client.file.get", &format!("{} bytes {}", data.len(), dev.id));
                say_file(&mut self.out, &FileMsg::FileGetOk { id, size: data.len() as u64 });
                self.files.down = Some(Download { id, data, at: 0 });
            }
            Ok(_) => self.file_err(id, "too_big", "that file is too big to send"),
            Err(e) => self.file_err(id, &e.code, &e.message),
        }
    }

    pub(super) fn file_put(&mut self, id: u32, path: &str, size: u64) {
        if self.files.up.is_some() {
            return self.file_err(id, "busy", "one upload at a time");
        }
        if size as usize > proto::MAX_FILE || size > proto::MAX_FILE as u64 {
            return self.file_err(id, "too_big", &format!("a file is at most {} MB", proto::MAX_FILE / (1024 * 1024)));
        }
        if iyakki_core_api::files::check_rel(path).map(|n| n.is_empty()).unwrap_or(true) {
            return self.file_err(id, "bad_request", "that is not a file name inside your home folder");
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let Some((person, dev)) = self.file_gate(id, &format!("save {name} ({size} bytes) in your home folder")) else { return };
        (self.ep.audit)(&person, "client.file.put.begin", &format!("{size} bytes {}", dev.id));
        self.files.up = Some(Upload { id, path: path.to_string(), size: size as usize, buf: Vec::with_capacity((size as usize).min(1 << 20)) });
        say_file(&mut self.out, &FileMsg::FilePutOk { id });
    }

    /// A chunk of the upload (the payload after the stream tag).
    pub(super) fn file_chunk(&mut self, payload: &[u8]) {
        let Some((id, bytes)) = proto::split_file_chunk(payload) else { return self.file_err(0, "bad_request", "a file chunk is too short") };
        let bad = match self.files.up.as_mut() {
            None => Some("no upload is open"),
            Some(u) if u.id != id => Some("that chunk belongs to no open upload"),
            Some(u) if u.buf.len() + bytes.len() > u.size => Some("more bytes came than the upload announced"),
            Some(u) => {
                u.buf.extend_from_slice(bytes);
                None
            }
        };
        if let Some(why) = bad {
            // the upload is dropped: what arrived is thrown away
            self.files.up = None;
            self.file_err(id, "bad_request", why);
        }
    }

    pub(super) fn file_end(&mut self, id: u32) {
        let Some(u) = self.files.up.take().filter(|u| u.id == id) else { return self.file_err(id, "bad_request", "no such upload is open") };
        if u.buf.len() != u.size {
            return self.file_err(id, "bad_request", "the upload ended before all its bytes came");
        }
        let (person, dev) = (self.person.clone().unwrap_or_default(), self.device.as_ref().map(|d| d.id.clone()).unwrap_or_default());
        let Some(screen) = self.screen.as_mut() else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
        match screen.files_save(&u.path, u.buf) {
            Ok(_) => {
                (self.ep.audit)(&person, "client.file.put", &format!("{} bytes {dev}", u.size));
                say_file(&mut self.out, &FileMsg::FileSaved { id, path: u.path });
            }
            Err(e) => self.file_err(id, &e.code, &e.message),
        }
    }

    pub(super) fn file_cancel(&mut self, id: u32) {
        if self.files.up.as_ref().is_some_and(|u| u.id == id) {
            self.files.up = None;
        }
        if self.files.down.as_ref().is_some_and(|d| d.id == id) {
            self.files.down = None;
        }
    }

    /// Called every turn of the session loop: a few chunks of the download that is going, then its end.
    pub(super) fn pump_files(&mut self) {
        let Some(d) = self.files.down.as_mut() else { return };
        for _ in 0..CHUNKS_PER_TURN {
            let end = (d.at + proto::FILE_CHUNK).min(d.data.len());
            if d.at < end {
                self.out.extend(ws::binary_frame(&proto::file_chunk(proto::STREAM_FILE_OUT, d.id, &d.data[d.at..end])));
                d.at = end;
            }
            if d.at >= d.data.len() {
                let id = d.id;
                self.files.down = None;
                say_file(&mut self.out, &FileMsg::FileDone { id });
                return;
            }
        }
    }
}
