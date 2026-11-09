//! The motion event store: when motion started and ended on a camera, in which zone, how strong, and one small thumbnail.
//!
//! Stored exactly like recordings (F-CAM-03): the same encrypted file format (`camrec_format`, kind `Events`) and the SAME per-camera
//! key in the owner's vault (`camrec.<id>`; it is made when motion watching is first turned on, if recording never made it). So the
//! events have the same protection (AES-256-XTS, encrypt-then-MAC, tampering and a wrong key found, a power cut loses only the last
//! record) and the same retention (the camera's `max_days`, plus a size cap). What someone with the disk can see: the camera id, the
//! day folders and the sizes and times of records (that hints at when something happened). What they cannot: zone names, scores,
//! thumbnails.
//!
//! Layout: `<data>/cameras/events/<camera id>/YYYY/MM/DD/HHMMSS.iev`. A new file starts when the store first writes after being idle,
//! after an hour or 256 KiB. Records: `flags` 0x10 start (JSON), 0x20 end (JSON), 0x30 person answer (JSON), 0x40 thumbnail
//! (8 bytes of event id, then a JPEG). An event is identified by its start time in milliseconds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::camrec_format::{self as fmt, RecKey, RecKind, Segment, SegmentWriter};
use crate::clock::Local;

pub const F_START: u8 = 0x10;
pub const F_END: u8 = 0x20;
pub const F_PERSON: u8 = 0x30;
pub const F_THUMB: u8 = 0x40;
const ROTATE_BYTES: u64 = 256 * 1024;
const ROTATE_MS: u64 = 3_600_000;
/// The most events (and thumbnails) of one camera kept, by size.
pub const EVENTS_MAX_MB: u64 = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The start time in milliseconds since 1970: the event's name.
    pub id: u64,
    pub start_ms: u64,
    /// `None`: not ended (in progress, or the computer stopped).
    pub end_ms: Option<u64>,
    pub zone: String,
    pub score: u8,
    pub peak: u8,
    /// `[left, top, right, bottom]`, 0.0 to 1.0.
    pub bbox: [f32; 4],
    /// `Some(true)` a person detector said person, `Some(false)` it found none, `None` not asked or not available.
    pub person: Option<bool>,
    pub has_thumb: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Rec {
    t: String,
    id: u64,
    #[serde(default)]
    ts: u64,
    #[serde(default)]
    zone: String,
    #[serde(default)]
    score: u8,
    #[serde(default)]
    bbox: [f32; 4],
    #[serde(default)]
    end: u64,
    #[serde(default)]
    peak: u8,
    #[serde(default)]
    person: Option<bool>,
}

struct Open {
    w: SegmentWriter,
    opened_ms: u64,
}

pub struct EventStore {
    root: PathBuf,
    open: Mutex<HashMap<String, Open>>,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl EventStore {
    pub fn new(data: &Path) -> EventStore {
        EventStore { root: data.join("cameras").join("events"), open: Mutex::new(HashMap::new()) }
    }

    pub fn dir_of(&self, cam: &str) -> PathBuf {
        self.root.join(cam)
    }

    fn append(&self, cam: &str, key: &RecKey, local: &Local, ts_ms: u64, flags: u8, data: &[u8]) -> Result<(), String> {
        if !safe_id(cam) {
            return Err("not a camera id".into());
        }
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if open.get(cam).is_some_and(|o| o.w.bytes >= ROTATE_BYTES || ts_ms.saturating_sub(o.opened_ms) >= ROTATE_MS) {
            open.remove(cam);
        }
        if !open.contains_key(cam) {
            let dir = self.dir_of(cam).join(format!("{:04}/{:02}/{:02}", local.year, local.month, local.day));
            let stem = format!("{:02}{:02}{:02}", local.hour, local.minute, local.second);
            let mut made = None;
            for n in 0..50 {
                let path = dir.join(if n == 0 { format!("{stem}.iev") } else { format!("{stem}-{n}.iev") });
                if !path.exists() {
                    made = Some(SegmentWriter::create(&path, key, RecKind::Events, ts_ms)?);
                    break;
                }
            }
            let w = made.ok_or("could not find a free file name for the events")?;
            open.insert(cam.to_string(), Open { w, opened_ms: ts_ms });
        }
        let o = open.get_mut(cam).ok_or("no event file")?;
        o.w.append(ts_ms, flags, data)?;
        o.w.sync();
        Ok(())
    }

    fn json(&self, cam: &str, key: &RecKey, local: &Local, ts: u64, flags: u8, r: &Rec) -> Result<(), String> {
        self.append(cam, key, local, ts, flags, &serde_json::to_vec(r).map_err(|e| e.to_string())?)
    }

    /// An event began. The thumbnail (a JPEG, already without masked areas) is stored with it.
    pub fn start(&self, cam: &str, key: &RecKey, local: &Local, ev: &Event, thumb: Option<&[u8]>) -> Result<(), String> {
        self.json(cam, key, local, ev.start_ms, F_START, &Rec { t: "s".into(), id: ev.id, ts: ev.start_ms, zone: ev.zone.clone(), score: ev.score, bbox: ev.bbox, ..Default::default() })?;
        if let Some(j) = thumb.filter(|j| !j.is_empty() && j.len() <= 256 * 1024) {
            let mut d = ev.id.to_le_bytes().to_vec();
            d.extend_from_slice(j);
            self.append(cam, key, local, ev.start_ms, F_THUMB, &d)?;
        }
        Ok(())
    }

    pub fn end(&self, cam: &str, key: &RecKey, local: &Local, id: u64, end_ms: u64, peak: u8) -> Result<(), String> {
        self.json(cam, key, local, end_ms, F_END, &Rec { t: "e".into(), id, end: end_ms, peak, ..Default::default() })
    }

    pub fn person(&self, cam: &str, key: &RecKey, local: &Local, id: u64, ts: u64, found: bool) -> Result<(), String> {
        self.json(cam, key, local, ts, F_PERSON, &Rec { t: "p".into(), id, ts, person: Some(found), ..Default::default() })
    }

    /// Close the open file of a camera (watching stopped); everything written is already on disk.
    pub fn close(&self, cam: &str) {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).remove(cam);
    }

    pub fn close_all(&self) {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn files(&self, cam: &str) -> Vec<PathBuf> {
        fn walk(d: &Path, out: &mut Vec<PathBuf>, depth: u32) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && depth < 4 {
                    walk(&p, out, depth + 1);
                } else if p.extension().is_some_and(|x| x == "iev") {
                    out.push(p);
                }
            }
        }
        let mut v = Vec::new();
        if safe_id(cam) {
            walk(&self.dir_of(cam), &mut v, 0);
        }
        v.sort();
        v
    }

    /// Events that started between `from_ms` and `to_ms` (inclusive), newest first, at most `limit`. Records that fail their check are
    /// skipped and counted in the second value.
    pub fn read(&self, cam: &str, key: &RecKey, from_ms: u64, to_ms: u64, limit: usize) -> Result<(Vec<Event>, usize), String> {
        let mut by_id: HashMap<u64, Event> = HashMap::new();
        let mut damaged = 0usize;
        for f in self.files(cam) {
            let Ok(sc) = fmt::scan(&f) else { continue };
            // a file holds records from its start to its last record; an event's end may be in a later file
            if sc.start_ms > to_ms.saturating_add(86_400_000) || sc.end_ms().saturating_add(86_400_000) < from_ms {
                continue;
            }
            let seg = match Segment::open(&f, key) {
                Ok(s) => s,
                Err(fmt::ReadErr::WrongKey) => return Err("the key in your vault does not open the motion events of this camera".into()),
                Err(_) => {
                    damaged += 1;
                    continue;
                }
            };
            for i in 0..seg.len() {
                let fr = match seg.frame(i) {
                    Ok(x) => x,
                    Err(_) => {
                        damaged += 1;
                        continue;
                    }
                };
                match fr.flags {
                    F_THUMB if fr.data.len() > 8 => {
                        let id = u64::from_le_bytes(fr.data[..8].try_into().unwrap_or([0; 8]));
                        if let Some(e) = by_id.get_mut(&id) {
                            e.has_thumb = true;
                        } else {
                            by_id.insert(id, Event { id, start_ms: id, end_ms: None, zone: String::new(), score: 0, peak: 0, bbox: [0.0; 4], person: None, has_thumb: true });
                        }
                    }
                    F_START | F_END | F_PERSON => {
                        let Ok(r) = serde_json::from_slice::<Rec>(&fr.data) else {
                            damaged += 1;
                            continue;
                        };
                        let e = by_id.entry(r.id).or_insert_with(|| Event { id: r.id, start_ms: r.id, end_ms: None, zone: String::new(), score: 0, peak: 0, bbox: [0.0; 4], person: None, has_thumb: false });
                        match r.t.as_str() {
                            "s" => {
                                e.start_ms = r.ts;
                                e.zone = r.zone;
                                e.score = r.score;
                                e.peak = r.score.max(e.peak);
                                e.bbox = r.bbox;
                            }
                            "e" => {
                                e.end_ms = Some(r.end);
                                e.peak = r.peak.max(e.peak);
                            }
                            "p" => e.person = r.person,
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut v: Vec<Event> = by_id.into_values().filter(|e| e.start_ms >= from_ms && e.start_ms <= to_ms && !e.zone.is_empty()).collect();
        v.sort_by(|a, b| b.start_ms.cmp(&a.start_ms));
        v.truncate(limit.clamp(1, 5000));
        Ok((v, damaged))
    }

    /// The thumbnail (a JPEG) of an event.
    pub fn thumb(&self, cam: &str, key: &RecKey, id: u64) -> Result<Option<Vec<u8>>, String> {
        for f in self.files(cam) {
            let Ok(sc) = fmt::scan(&f) else { continue };
            if sc.start_ms > id.saturating_add(120_000) || sc.end_ms() + 120_000 < id {
                continue;
            }
            let seg = Segment::open(&f, key).map_err(|e| e.to_string())?;
            for i in 0..seg.len() {
                if seg.scan.frames[i].flags != F_THUMB {
                    continue;
                }
                let fr = seg.frame(i).map_err(|e| e.to_string())?;
                if fr.data.len() > 8 && u64::from_le_bytes(fr.data[..8].try_into().unwrap_or([0; 8])) == id {
                    return Ok(Some(fr.data[8..].to_vec()));
                }
            }
        }
        Ok(None)
    }

    /// Retention: files whose last record is older than `max_days` (0: no age limit), then oldest first until the camera's events are
    /// under `max_bytes`. The file being written is never removed. Returns (files, bytes) removed.
    pub fn enforce(&self, cam: &str, now_ms: u64, max_days: u32, max_bytes: u64) -> (usize, u64) {
        let writing: Option<u64> = self.open.lock().unwrap_or_else(|e| e.into_inner()).get(cam).map(|o| o.w.start_ms);
        let mut list: Vec<(u64, u64, PathBuf, u64)> = Vec::new();
        for f in self.files(cam) {
            let Ok(sc) = fmt::scan(&f) else { continue };
            if writing == Some(sc.start_ms) {
                continue;
            }
            list.push((sc.start_ms, sc.end_ms(), f, sc.file_len));
        }
        list.sort_by_key(|x| x.0);
        let (mut n, mut bytes) = (0usize, 0u64);
        let mut total: u64 = list.iter().map(|x| x.3).sum();
        for (_, end, path, len) in list {
            let old = max_days > 0 && now_ms.saturating_sub(end) > max_days as u64 * 86_400_000;
            if old || (max_bytes > 0 && total > max_bytes) {
                if std::fs::remove_file(&path).is_ok() {
                    n += 1;
                    bytes += len;
                    total = total.saturating_sub(len);
                    // empty day, month and year folders go too (remove_dir fails when something is left)
                    let mut p = path.parent().map(Path::to_path_buf);
                    for _ in 0..3 {
                        match p {
                            Some(d) if std::fs::remove_dir(&d).is_ok() => p = d.parent().map(Path::to_path_buf),
                            _ => break,
                        }
                    }
                }
            }
        }
        (n, bytes)
    }

    /// Camera ids that have event folders (for retention of cameras that were removed).
    pub fn cameras(&self) -> Vec<String> {
        std::fs::read_dir(&self.root).map(|rd| rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| safe_id(n)).collect()).unwrap_or_default()
    }

    /// Total bytes of a camera's events.
    pub fn bytes(&self, cam: &str) -> u64 {
        self.files(cam).iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Zone;

    fn tmp(name: &str) -> PathBuf {
        crate::testdir::unique("camevents", name)
    }
    fn local(ms: u64) -> Local {
        Zone::utc().local((ms / 1000) as i64)
    }
    fn ev(id: u64, zone: &str, score: u8) -> Event {
        Event { id, start_ms: id, end_ms: None, zone: zone.into(), score, peak: score, bbox: [0.1, 0.2, 0.3, 0.4], person: None, has_thumb: false }
    }
    const T0: u64 = 1_700_000_000_000;

    #[test]
    fn events_round_trip_with_zone_score_box_end_person_and_thumbnail() {
        let d = tmp("round");
        let (s, key) = (EventStore::new(&d), RecKey::generate().unwrap());
        s.start("c1", &key, &local(T0), &ev(T0, "Driveway", 42), Some(&[0xFF, 0xD8, 9, 9, 0xFF, 0xD9])).unwrap();
        s.end("c1", &key, &local(T0 + 9000), T0, T0 + 9000, 77).unwrap();
        s.person("c1", &key, &local(T0 + 9000), T0, T0 + 9000, true).unwrap();
        s.start("c1", &key, &local(T0 + 60_000), &ev(T0 + 60_000, "Porch", 10), None).unwrap();
        let (v, damaged) = s.read("c1", &key, 0, u64::MAX, 100).unwrap();
        assert_eq!(damaged, 0);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].zone, "Porch", "newest first");
        assert!(v[0].end_ms.is_none() && !v[0].has_thumb, "not ended: shown as in progress or interrupted");
        let first = &v[1];
        assert_eq!((first.zone.as_str(), first.score, first.peak, first.end_ms, first.person, first.has_thumb), ("Driveway", 42, 77, Some(T0 + 9000), Some(true), true));
        assert_eq!(first.bbox, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(s.thumb("c1", &key, T0).unwrap().unwrap(), vec![0xFF, 0xD8, 9, 9, 0xFF, 0xD9]);
        assert!(s.thumb("c1", &key, T0 + 60_000).unwrap().is_none());
        // a time window and a limit
        assert_eq!(s.read("c1", &key, T0 + 1000, u64::MAX, 100).unwrap().0.len(), 1);
        assert_eq!(s.read("c1", &key, 0, u64::MAX, 1).unwrap().0.len(), 1);
    }

    #[test]
    fn the_files_hold_no_readable_zone_names_or_pictures_and_a_wrong_key_is_refused() {
        let d = tmp("secret");
        let (s, key) = (EventStore::new(&d), RecKey::generate().unwrap());
        let marker = b"ZZMARKER-JPEG-BYTES-ZZ";
        s.start("c1", &key, &local(T0), &ev(T0, "Secretgarden", 42), Some(marker)).unwrap();
        s.close("c1");
        let files = s.files("c1");
        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().ends_with(".iev") && files[0].to_string_lossy().contains("/2023/11/14/"));
        let raw = std::fs::read(&files[0]).unwrap();
        let has = |h: &[u8], n: &[u8]| h.windows(n.len()).any(|w| w == n);
        assert!(!has(&raw, b"Secretgarden") && !has(&raw, marker) && !has(&raw, b"\"zone\""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let other = RecKey::generate().unwrap();
        assert!(s.read("c1", &other, 0, u64::MAX, 10).unwrap_err().contains("does not open"));
    }

    #[test]
    fn a_changed_byte_is_found_and_never_shown_and_a_power_cut_loses_only_the_last_record() {
        let d = tmp("tamper");
        let (s, key) = (EventStore::new(&d), RecKey::generate().unwrap());
        for k in 0..3u64 {
            s.start("c1", &key, &local(T0), &ev(T0 + k * 1000, "Z", 5), None).unwrap();
        }
        s.close("c1");
        let f = s.files("c1").remove(0);
        let good = std::fs::read(&f).unwrap();
        // flip one byte in the middle of the second record
        let mut bad = good.clone();
        let mid = good.len() / 2;
        bad[mid] ^= 0x01;
        std::fs::write(&f, &bad).unwrap();
        let (v, damaged) = s.read("c1", &key, 0, u64::MAX, 10).unwrap();
        assert!(damaged >= 1 && v.len() == 2, "the damaged record is skipped and counted: {} events, {damaged} damaged", v.len());
        // cut the file in the middle of the last record
        std::fs::write(&f, &good[..good.len() - 20]).unwrap();
        let (v, _) = s.read("c1", &key, 0, u64::MAX, 10).unwrap();
        assert_eq!(v.len(), 2, "the two complete records read");
    }

    #[test]
    fn retention_removes_old_files_then_the_oldest_over_the_size_cap_but_not_the_one_being_written() {
        let d = tmp("keep");
        let (s, key) = (EventStore::new(&d), RecKey::generate().unwrap());
        let day = 86_400_000u64;
        // three files on three different days, each closed
        for k in 0..3u64 {
            let t = T0 + k * 10 * day;
            s.start("c1", &key, &local(t), &ev(t, "Z", 5), Some(&vec![7u8; 4000])).unwrap();
            s.end("c1", &key, &local(t + 5000), t, t + 5000, 5).unwrap();
            s.close("c1");
        }
        // the one being written now (later than the others)
        let now = T0 + 25 * day;
        s.start("c1", &key, &local(now), &ev(now, "Now", 5), None).unwrap();
        assert_eq!(s.files("c1").len(), 4);
        // 14 days: the files from day 0 and day 10 are older than 14 days at day 25; the one from day 20 stays
        let (n, bytes) = s.enforce("c1", now, 14, 0);
        assert_eq!(n, 2);
        assert!(bytes > 4000);
        assert_eq!(s.files("c1").len(), 2);
        // a tiny size cap removes the older closed file but never the open one
        let (n, _) = s.enforce("c1", now, 0, 1);
        assert_eq!(n, 1);
        let (v, _) = s.read("c1", &key, 0, u64::MAX, 10).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].zone, "Now");
        // no age limit and no size cap: nothing goes
        assert_eq!(s.enforce("c1", now + 1000 * day, 0, 0), (0, 0));
        assert!(s.bytes("c1") > 0 && s.cameras() == vec!["c1".to_string()]);
    }

    #[test]
    fn a_camera_id_cannot_climb_out_of_the_folder() {
        let d = tmp("ids");
        let (s, key) = (EventStore::new(&d), RecKey::generate().unwrap());
        assert!(s.start("../x", &key, &local(T0), &ev(T0, "Z", 5), None).is_err());
        assert!(s.start("a/b", &key, &local(T0), &ev(T0, "Z", 5), None).is_err());
        assert!(s.read("../x", &key, 0, u64::MAX, 5).unwrap().0.is_empty());
    }
}
