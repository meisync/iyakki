//! The state machine of the USB writer WINDOW (F-PKG-08): drop the image, pick the stick, press one button. It holds no window and
//! draws nothing: a renderer (the shell's own, or any other) shows [`View`] and sends [`Event`]s, so every rule (which sticks may be
//! picked, what must be ticked first, what happens when the stick is pulled) exists once and is tested without a screen.
//!
//! Pages: Image, Stick, Confirm, Writing, Done, Failed. The same rules as the terminal writer ([`crate::drives::verdict`]): a system
//! disk, a disk with the running system, swap, a write-protected or empty drive and the stick the image lives on are never offered
//! as choices; an internal disk is offered only after "show all disks" and needs the extra tick. No typing: the one button, and a
//! tick when the plan erases something the person might not expect (owner rule, docs/INSTALLER.md).

use std::path::{Path, PathBuf};

use crate::cli::items_for;
use crate::drives::{Drive, Verdict};
use crate::ui::{human_size, Item};
use crate::writer::{ImageInfo, ImageKind, Phase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Image,
    Stick,
    Confirm,
    Writing,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// A file dropped on the window, or chosen in a file picker.
    ImageChosen(PathBuf),
    /// The fresh list of drives (after a plug or unplug, or on the refresh the renderer does every second or two).
    Drives(Vec<Drive>),
    ShowAllDisks(bool),
    /// A row of the stick list was clicked (index into [`View::rows`]).
    PickRow(usize),
    /// The tick box on the confirm page.
    Acknowledge(bool),
    /// The one button.
    Button,
    Back,
    /// Progress from the writer thread.
    Progress { phase: Phase, done: u64, total: u64 },
    Finished(Result<(), String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub detail: String,
    pub selectable: bool,
    pub selected: bool,
    /// Why it cannot be chosen.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub label: &'static str,
    pub enabled: bool,
}

/// What the renderer draws.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub page: Page,
    pub title: String,
    pub lines: Vec<String>,
    pub rows: Vec<Row>,
    pub tick: Option<(String, bool)>,
    pub progress: Option<(String, f64)>,
    pub button: Option<Button>,
    pub can_go_back: bool,
}

/// What the machine asks the program to do. The writer is started by the program, never by the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Start writing `image` to `drive` (a whole-stick write, with the install layout when `install`).
    Write { image: PathBuf, drive: Drive, install: bool },
    Close,
}

pub struct Window {
    page: Page,
    image: Option<ImageInfo>,
    image_error: Option<String>,
    drives: Vec<Drive>,
    show_all: bool,
    chosen: Option<String>,
    ack: bool,
    progress: Option<(Phase, u64, u64)>,
    failure: Option<String>,
    note: Option<String>,
    inspect: fn(&Path) -> Result<ImageInfo, String>,
}

impl Window {
    pub fn new() -> Window {
        Window::with_inspector(crate::writer::inspect_image)
    }

    pub fn with_inspector(inspect: fn(&Path) -> Result<ImageInfo, String>) -> Window {
        Window { page: Page::Image, image: None, image_error: None, drives: vec![], show_all: false, chosen: None, ack: false, progress: None, failure: None, note: None, inspect }
    }

    pub fn page(&self) -> Page {
        self.page
    }

    fn items(&self) -> Vec<Item> {
        match &self.image {
            Some(i) => items_for(&self.drives, &i.path, self.show_all),
            None => vec![],
        }
    }

    fn chosen_item(&self) -> Option<Item> {
        let c = self.chosen.as_ref()?;
        self.items().into_iter().find(|i| &i.drive.path == c)
    }

    fn needs_ack(&self) -> bool {
        self.chosen_item().is_some_and(|i| i.verdict == Verdict::Internal)
    }

    /// Install layout for an install template (the stick boots and keeps data); a plain copy otherwise.
    fn install_mode(&self) -> bool {
        self.image.as_ref().is_some_and(|i| i.kind == ImageKind::InstallTemplate)
    }

    pub fn handle(&mut self, e: Event) -> Action {
        match e {
            Event::ImageChosen(p) => {
                if matches!(self.page, Page::Writing) {
                    return Action::None;
                }
                match (self.inspect)(&p) {
                    Ok(i) if i.kind == ImageKind::Unknown => {
                        self.image = None;
                        self.image_error = Some(format!("{} is not an IYAKKI image (it is not an ISO, a disk image or an install template).", p.display()));
                        self.page = Page::Image;
                    }
                    Ok(i) => {
                        self.image = Some(i);
                        self.image_error = None;
                        self.chosen = None;
                        self.ack = false;
                        self.page = Page::Stick;
                    }
                    Err(e) => {
                        self.image = None;
                        self.image_error = Some(e);
                        self.page = Page::Image;
                    }
                }
            }
            Event::Drives(d) => {
                self.drives = d;
                // the stick was pulled (or became unusable) after it was chosen: back to the list, with a note
                if self.chosen.is_some() && self.chosen_item().is_none_or(|i| matches!(i.verdict, Verdict::Refused(_))) && matches!(self.page, Page::Confirm | Page::Stick) {
                    self.chosen = None;
                    self.ack = false;
                    self.note = Some("The stick you chose is gone or can no longer be written. Choose again.".into());
                    self.page = Page::Stick;
                }
            }
            Event::ShowAllDisks(b) => {
                if matches!(self.page, Page::Stick) {
                    self.show_all = b;
                    if !b && self.chosen_item().is_none() {
                        self.chosen = None;
                    }
                }
            }
            Event::PickRow(n) => {
                if self.page == Page::Stick {
                    if let Some(it) = self.items().get(n) {
                        if !matches!(it.verdict, Verdict::Refused(_)) {
                            self.chosen = Some(it.drive.path.clone());
                            self.note = None;
                        }
                    }
                }
            }
            Event::Acknowledge(b) => {
                if self.page == Page::Confirm {
                    self.ack = b;
                }
            }
            Event::Back => match self.page {
                Page::Stick => self.page = Page::Image,
                Page::Confirm => {
                    self.ack = false;
                    self.page = Page::Stick;
                }
                Page::Failed | Page::Done => {
                    self.failure = None;
                    self.progress = None;
                    self.ack = false;
                    self.page = Page::Stick;
                }
                _ => {}
            },
            Event::Button => match self.page {
                Page::Stick if self.chosen_item().is_some() => {
                    self.ack = false;
                    self.page = Page::Confirm;
                }
                Page::Confirm => {
                    if let Some(it) = self.chosen_item() {
                        if self.needs_ack() && !self.ack {
                            return Action::None;
                        }
                        if matches!(it.verdict, Verdict::Refused(_)) {
                            return Action::None;
                        }
                        let image = self.image.as_ref().map(|i| i.path.clone()).unwrap_or_default();
                        self.page = Page::Writing;
                        self.progress = Some((Phase::Checking, 0, 1));
                        return Action::Write { image, drive: it.drive, install: self.install_mode() };
                    }
                }
                Page::Done => return Action::Close,
                Page::Failed => {
                    self.failure = None;
                    self.page = Page::Confirm;
                }
                _ => {}
            },
            Event::Progress { phase, done, total } => {
                if self.page == Page::Writing {
                    self.progress = Some((phase, done, total));
                }
            }
            Event::Finished(r) => {
                if self.page == Page::Writing {
                    match r {
                        Ok(()) => self.page = Page::Done,
                        Err(e) => {
                            self.failure = Some(e);
                            self.page = Page::Failed;
                        }
                    }
                }
            }
        }
        Action::None
    }

    pub fn view(&self) -> View {
        let image_line = self.image.as_ref().map(|i| format!("Image: {} ({})", i.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), human_size(i.size)));
        match self.page {
            Page::Image => View {
                page: self.page,
                title: "Write IYAKKI to a USB stick".into(),
                lines: [Some("Drop the IYAKKI image file here, or choose it.".to_string()), self.image_error.clone()].into_iter().flatten().collect(),
                rows: vec![],
                tick: None,
                progress: None,
                button: Some(Button { label: "Choose image", enabled: true }),
                can_go_back: false,
            },
            Page::Stick => {
                let items = self.items();
                let rows = items
                    .iter()
                    .map(|i| {
                        let d = &i.drive;
                        let kind = if d.usb { "USB" } else if d.removable { "removable" } else { "internal disk" };
                        let refused = if let Verdict::Refused(r) = &i.verdict { Some(r.clone()) } else { None };
                        Row { label: format!("{} {}", d.model, human_size(d.size_bytes)), detail: format!("{} - {}", d.path, kind), selectable: refused.is_none(), selected: self.chosen.as_deref() == Some(d.path.as_str()), reason: refused }
                    })
                    .collect::<Vec<_>>();
                let mut lines: Vec<String> = image_line.into_iter().collect();
                lines.push(if rows.is_empty() { "Plug in a USB stick. It appears here by itself.".into() } else { "Choose the stick to write to.".into() });
                lines.extend(self.note.clone());
                View { page: self.page, title: "Choose the stick".into(), lines, rows, tick: None, progress: None, button: Some(Button { label: "Next", enabled: self.chosen_item().is_some() }), can_go_back: true }
            }
            Page::Confirm => {
                let it = self.chosen_item();
                let mut lines: Vec<String> = image_line.into_iter().collect();
                if let Some(it) = &it {
                    lines.push(format!("Stick: {} {} ({})", it.drive.model, human_size(it.drive.size_bytes), it.drive.path));
                    lines.push(if self.install_mode() { "The stick gets IYAKKI with its own boot menu and keeps a data area for your files.".into() } else { "The image is copied to the stick as it is.".into() });
                    if !it.drive.mounts.is_empty() {
                        lines.push(format!("{} volume(s) on it will be unmounted first.", it.drive.mounts.len()));
                    }
                }
                lines.push("Everything on this stick will be erased and cannot be recovered.".into());
                let ack = self.needs_ack();
                View {
                    page: self.page,
                    title: "Ready to write".into(),
                    lines,
                    rows: vec![],
                    tick: ack.then(|| ("This is an internal disk, not a USB stick. I checked that it is the one I mean.".to_string(), self.ack)),
                    progress: None,
                    button: Some(Button { label: "Write", enabled: it.is_some() && (!ack || self.ack) }),
                    can_go_back: true,
                }
            }
            Page::Writing => {
                let (ph, d, t) = self.progress.unwrap_or((Phase::Checking, 0, 1));
                View { page: self.page, title: "Writing".into(), lines: vec!["Do not unplug the stick.".into()], rows: vec![], tick: None, progress: Some((ph.label().to_string(), if t == 0 { 1.0 } else { d as f64 / t as f64 })), button: None, can_go_back: false }
            }
            Page::Done => View { page: self.page, title: "Done".into(), lines: vec!["The stick is written and checked. You can unplug it and start a computer from it.".into()], rows: vec![], tick: None, progress: None, button: Some(Button { label: "Close", enabled: true }), can_go_back: true },
            Page::Failed => View { page: self.page, title: "The stick was not written".into(), lines: vec![self.failure.clone().unwrap_or_default(), "Nothing else was changed. You can try again or choose another stick.".into()], rows: vec![], tick: None, progress: None, button: Some(Button { label: "Try again", enabled: true }), can_go_back: true },
        }
    }
}

impl Default for Window {
    fn default() -> Self {
        Window::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(p: &Path) -> Result<ImageInfo, String> {
        match p.to_string_lossy().as_ref() {
            "/img/iyakki.iso" => Ok(ImageInfo { path: p.into(), size: 900_000_000, kind: ImageKind::Iso9660 }),
            "/img/install.img" => Ok(ImageInfo { path: p.into(), size: 2_000_000_000, kind: ImageKind::InstallTemplate }),
            "/img/notes.txt" => Ok(ImageInfo { path: p.into(), size: 10, kind: ImageKind::Unknown }),
            _ => Err(format!("{}: No such file", p.display())),
        }
    }

    fn drive(name: &str, usb: bool) -> Drive {
        Drive { path: format!("/dev/{name}"), name: name.into(), model: "Cruzer".into(), size_bytes: 16_000_000_000, removable: usb, usb, read_only: false, mounts: vec![], swap: false }
    }

    fn sys() -> Drive {
        let mut d = drive("nvme0n1", false);
        d.mounts = vec![("/dev/nvme0n1p2".into(), "/".into())];
        d
    }

    fn win() -> Window {
        Window::with_inspector(fake)
    }

    #[test]
    fn an_unknown_or_missing_file_stays_on_the_first_page_and_says_why() {
        let mut w = win();
        assert_eq!(w.page(), Page::Image);
        w.handle(Event::ImageChosen("/img/notes.txt".into()));
        assert_eq!(w.page(), Page::Image);
        assert!(w.view().lines.iter().any(|l| l.contains("is not an IYAKKI image")));
        w.handle(Event::ImageChosen("/img/missing.iso".into()));
        assert!(w.view().lines.iter().any(|l| l.contains("No such file")));
        w.handle(Event::ImageChosen("/img/iyakki.iso".into()));
        assert_eq!(w.page(), Page::Stick);
    }

    #[test]
    fn only_usable_sticks_can_be_picked_the_button_waits_for_a_pick_and_refused_rows_say_why() {
        let mut w = win();
        w.handle(Event::ImageChosen("/img/iyakki.iso".into()));
        w.handle(Event::Drives(vec![sys(), drive("sdb", true)]));
        let v = w.view();
        assert_eq!(v.rows.len(), 1, "the internal disk is not even listed until all disks are shown");
        assert!(!v.button.clone().unwrap().enabled);
        w.handle(Event::ShowAllDisks(true));
        let v = w.view();
        assert_eq!(v.rows.len(), 2);
        let sysrow = v.rows.iter().position(|r| r.reason.is_some()).unwrap();
        assert!(v.rows[sysrow].reason.as_ref().unwrap().contains("running system") && !v.rows[sysrow].selectable);
        w.handle(Event::PickRow(sysrow));
        assert!(!w.view().button.unwrap().enabled, "a refused row cannot be picked");
        let usb = v.rows.iter().position(|r| r.selectable).unwrap();
        w.handle(Event::PickRow(usb));
        assert!(w.view().rows[usb].selected && w.view().button.unwrap().enabled);
    }

    #[test]
    fn the_one_button_goes_stick_confirm_write_and_asks_the_program_to_write_exactly_once() {
        let mut w = win();
        w.handle(Event::ImageChosen("/img/install.img".into()));
        w.handle(Event::Drives(vec![drive("sdb", true)]));
        w.handle(Event::PickRow(0));
        assert_eq!(w.handle(Event::Button), Action::None);
        assert_eq!(w.page(), Page::Confirm);
        let v = w.view();
        assert!(v.tick.is_none() && v.button.clone().unwrap().label == "Write" && v.lines.iter().any(|l| l.contains("erased")) && v.lines.iter().any(|l| l.contains("keeps a data area")));
        let a = w.handle(Event::Button);
        assert!(matches!(&a, Action::Write { install: true, drive, .. } if drive.name == "sdb"), "{a:?}");
        assert_eq!(w.page(), Page::Writing);
        assert_eq!(w.handle(Event::Button), Action::None, "a second press while writing does nothing");
        assert_eq!(w.handle(Event::ImageChosen("/img/iyakki.iso".into())), Action::None);
        assert_eq!(w.page(), Page::Writing, "nothing can change while writing");
        w.handle(Event::Progress { phase: Phase::Writing, done: 50, total: 100 });
        assert_eq!(w.view().progress, Some(("Writing".to_string(), 0.5)));
        w.handle(Event::Finished(Ok(())));
        assert_eq!(w.page(), Page::Done);
        assert_eq!(w.handle(Event::Button), Action::Close);
    }

    #[test]
    fn an_internal_disk_needs_the_tick_and_a_plain_iso_is_copied_as_it_is() {
        let mut w = win();
        w.handle(Event::ImageChosen("/img/iyakki.iso".into()));
        let mut inner = drive("sdc", false);
        inner.removable = false;
        w.handle(Event::Drives(vec![inner]));
        w.handle(Event::ShowAllDisks(true));
        w.handle(Event::PickRow(0));
        w.handle(Event::Button);
        let v = w.view();
        assert!(v.tick.as_ref().is_some_and(|t| !t.1) && !v.button.unwrap().enabled);
        assert_eq!(w.handle(Event::Button), Action::None, "no tick, no write");
        assert_eq!(w.page(), Page::Confirm);
        w.handle(Event::Acknowledge(true));
        assert!(w.view().button.unwrap().enabled);
        assert!(matches!(w.handle(Event::Button), Action::Write { install: false, .. }));
    }

    #[test]
    fn a_stick_pulled_before_the_write_sends_the_person_back_to_the_list_with_a_note() {
        let mut w = win();
        w.handle(Event::ImageChosen("/img/iyakki.iso".into()));
        w.handle(Event::Drives(vec![drive("sdb", true)]));
        w.handle(Event::PickRow(0));
        w.handle(Event::Button);
        assert_eq!(w.page(), Page::Confirm);
        w.handle(Event::Drives(vec![]));
        assert_eq!(w.page(), Page::Stick);
        assert!(w.view().lines.iter().any(|l| l.contains("gone")));
        assert!(!w.view().button.unwrap().enabled);
        // a replugged stick with the same path is a new choice, not a remembered one
        w.handle(Event::Drives(vec![drive("sdb", true)]));
        assert!(!w.view().button.unwrap().enabled);
    }

    #[test]
    fn a_failed_write_says_why_and_try_again_returns_to_the_confirm_page_with_the_tick_cleared() {
        let mut w = win();
        w.handle(Event::ImageChosen("/img/iyakki.iso".into()));
        w.handle(Event::Drives(vec![drive("sdb", true)]));
        w.handle(Event::PickRow(0));
        w.handle(Event::Button);
        w.handle(Event::Button);
        w.handle(Event::Finished(Err("the drive does not hold what was written".into())));
        assert_eq!(w.page(), Page::Failed);
        assert!(w.view().lines[0].contains("does not hold"));
        w.handle(Event::Button);
        assert_eq!(w.page(), Page::Confirm);
        w.handle(Event::Back);
        w.handle(Event::Back);
        assert_eq!(w.page(), Page::Image);
    }

    #[test]
    fn the_stick_the_image_lives_on_is_refused() {
        let mut w = win();
        w.handle(Event::ImageChosen("/img/iyakki.iso".into()));
        let mut holder = drive("sdd", true);
        holder.mounts = vec![("/dev/sdd1".into(), "/img".into())];
        w.handle(Event::Drives(vec![holder]));
        let v = w.view();
        assert!(v.rows[0].reason.as_ref().is_some_and(|r| r.contains("image file")));
    }
}
