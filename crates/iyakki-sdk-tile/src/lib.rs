//! An SDK app as a WASM tile (F-SDK-23). The same source that runs in the shell, in the browser preview and in the SDK harness is wrapped
//! in [`SdkTile`], which is an `iyakki_tile_kit::App`: tile events go in (pointer, keys, text, ticks, resize), the app's display list
//! comes out mapped onto the tile ABI ([`convert`]). The app reaches the machine only through ports, and in the tile the ports are the
//! core's, over the tile's one door, `call(domain, method, json)` ([`ports`]): the host checks the manifest's capability before anything
//! is forwarded, the core asks the person where it must, and the call is logged.
//!
//! * [`SdkTile`]: the adapter. Time is the sum of the `Tick` events (the only clock a tile has), so a tap, a long press and a swipe
//!   behave as on the device. A tap is resolved by the SDK host against its own hit regions; the ABI's `Event::Hit` for the same tap is
//!   dropped so the app is not told twice; a `Hit` that no pointer pair came before (a keyboard walk, switch access) is an action.
//! * `guest` (wasm32 only): the six exports of the tile ABI over the SAMPLE app with core-backed storage and dock.
//!
//! Not here: names for a screen reader (the ABI's `Access` operation needs ABI 2 and the tile-kit guest speaks ABI 1), a pinch event
//! (the ABI has one pointer), the shell's pane manager and the wallpaper port.
use iyakki_tile_abi::{measure_text, DisplayList as Tile, Event};
use iyakki_tile_kit::App;
use iyakki_ui_sdk::app::UiApp;
use iyakki_ui_sdk::canvas::TextStyle;
use iyakki_ui_sdk::desktop::{Arrangement, DockPort, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, Pointer, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList as Sdk, ListCanvas};
use iyakki_ui_sdk::look::Look;
use std::cell::{Cell, RefCell};

pub mod convert;
pub mod ports;

/// The tile kit, so an app crate that builds as a tile depends on this crate alone.
pub use iyakki_tile_kit as kit;
pub use iyakki_ui_sdk::ports::StoragePort;

/// The six exports of the tile ABI over an SDK app, for the wasm32 build of an app crate:
/// `iyakki_sdk_tile::export_sdk_tile!(MyApp::with_storage);` where the path is a `fn(Box<dyn StoragePort>) -> MyApp`. The app is given
/// the core-backed storage port and its dock port is the core-backed one; both go through the tile's `call`, so the manifest decides.
#[macro_export]
macro_rules! export_sdk_tile {
    ($make:expr) => {
        fn __iyakki_make_tile(src: $crate::kit::guest::HostSource) -> Box<dyn $crate::kit::App> {
            let door = $crate::ports::Shared::new(src);
            let storage: Box<dyn $crate::StoragePort> = Box::new($crate::ports::CoreStorage::new(door.clone()));
            let make: fn(Box<dyn $crate::StoragePort>) -> _ = $make;
            Box::new($crate::SdkTile::new(make(storage), Box::new($crate::ports::CoreDock::new(door))))
        }
        $crate::kit::export_tile!(__iyakki_make_tile);
    };
}

#[cfg(all(target_arch = "wasm32", feature = "sample-guest"))]
mod guest;

/// The surface the tile starts with, before the host's first `Resize`.
pub const START_SIZE: (u32, u32) = (360, 740);

/// The tile ABI's key codes (docs/development/TILE-ABI.md section 3).
fn key(code: u32) -> Option<UiKey> {
    Some(match code {
        8 => UiKey::Backspace,
        9 => UiKey::Tab,
        13 => UiKey::Enter,
        27 => UiKey::Esc,
        127 => UiKey::Delete,
        0x100 => UiKey::Left,
        0x101 => UiKey::Right,
        0x102 => UiKey::Up,
        0x103 => UiKey::Down,
        c @ 32..=126 => UiKey::Char(char::from_u32(c)?),
        _ => return None,
    })
}

/// Text measured with the ABI's own table, so the app lays out in the tile exactly as the host will draw it.
fn measure(size: f32, s: &str, style: TextStyle) -> f32 {
    measure_text(convert::face(style), size.round().max(1.0) as u32, s) as f32
}

pub struct SdkTile<A: UiApp> {
    host: RefCell<Host<A>>,
    now_ms: Cell<u64>,
    /// The action a pointer release just made: the ABI's `Hit` for the same tap is not a second one.
    tapped: Cell<Option<u32>>,
    /// The printable key just pressed: the `TextInput` that repeats it is not typed twice.
    typed: Cell<Option<char>>,
    /// Why the last frame could not be given (a plain sentence), `None` when it could.
    pub refused: RefCell<Option<String>>,
}

impl<A: UiApp> SdkTile<A> {
    /// `dock` is the app's dock port: [`ports::CoreDock`] in the real tile, `MemDock` in a test.
    pub fn new(app: A, dock: Box<dyn DockPort>) -> SdkTile<A> {
        SdkTile::with_wallpaper(app, dock, Box::new(MemWallpaper::default()))
    }

    /// As [`SdkTile::new`] with the app's wallpaper port given: [`ports::CoreWallpaper`] in the real tile.
    pub fn with_wallpaper(app: A, dock: Box<dyn DockPort>, wp: Box<dyn iyakki_ui_sdk::desktop::WallpaperPort>) -> SdkTile<A> {
        let host = Host::new(app, Look::default(), Arrangement::single(START_SIZE.0, START_SIZE.1), dock, wp);
        SdkTile { host: RefCell::new(host), now_ms: Cell::new(0), tapped: Cell::new(None), typed: Cell::new(None), refused: RefCell::new(None) }
    }

    /// True while the app has a text field in focus: the host shows the on-screen keyboard (the SDK's keyboard hook).
    pub fn wants_keyboard(&self) -> bool {
        self.host.borrow().wants_keyboard()
    }

    /// The SDK frame as the app drew it (before the mapping), laid out with the ABI's text table.
    pub fn sdk_frame(&self) -> Sdk {
        let mut host = self.host.borrow_mut();
        let (w, h) = (host.pane.w, host.pane.h);
        let mut c = ListCanvas::with_measure(w, h, Box::new(measure));
        host.draw(&mut c);
        let list = c.finish();
        host.set_last(list.clone());
        list
    }

    pub fn with_app<R>(&self, f: impl FnOnce(&mut A) -> R) -> R {
        f(&mut self.host.borrow_mut().app)
    }
}

impl<A: UiApp> App for SdkTile<A> {
    fn event(&mut self, e: &Event) -> bool {
        let mut host = self.host.borrow_mut();
        let was_typed = self.typed.take();
        let now = self.now_ms.get();
        let pointer = |x: i32, y: i32, phase| Pointer { id: 0, x: x as f32, y: y as f32, phase, t_ms: now };
        match e {
            Event::PointerDown { x, y, button: 0 } => {
                self.tapped.set(None);
                host.pointer(pointer(*x, *y, Phase::Down));
            }
            Event::PointerMove { x, y } => {
                host.pointer(pointer(*x, *y, Phase::Move));
            }
            Event::PointerUp { x, y, button: 0 } => {
                // what the SDK host will make of this release: the action of the hit region a tap lands on
                let hit = host.action_at(*x as f32, *y as f32);
                host.pointer(pointer(*x, *y, Phase::Up));
                self.tapped.set(hit);
            }
            Event::Hit { id } if *id > 0 => {
                if self.tapped.take() == Some(id - 1) {
                    return false;
                }
                host.event(&UiEvent::Action(id - 1));
            }
            Event::Key { code, mods, down: true } => {
                // a shortcut with ctrl, alt or meta is the host's, not the app's; shift only makes the letter
                let Some(k) = key(*code).filter(|k| mods & 0b1110 == 0 || !matches!(k, UiKey::Char(_))) else { return false };
                if let UiKey::Char(c) = k {
                    self.typed.set(Some(c));
                }
                host.event(&UiEvent::Key(k));
            }
            Event::TextInput { text } => {
                let mut it = text.chars();
                if was_typed.is_some() && it.next() == was_typed && it.next().is_none() {
                    return false;
                }
                host.event(&UiEvent::Text(text.clone()));
            }
            Event::Tick { dt_ms } => {
                self.now_ms.set(now.saturating_add(u64::from(*dt_ms)));
                host.tick(self.now_ms.get());
            }
            Event::Resize { w, h, .. } => host.resize((*w).max(1) as f32, (*h).max(1) as f32),
            // the Look comes from the SDK's own theme and the person's settings through the core, not from the page
            Event::Theme { .. } => return false,
            _ => return false,
        }
        true
    }

    fn frame(&self) -> Tile {
        let sdk = self.sdk_frame();
        match convert::to_tile(&sdk) {
            Ok(t) => {
                *self.refused.borrow_mut() = None;
                t
            }
            Err(e) => {
                *self.refused.borrow_mut() = Some(e.to_string());
                Tile::new()
            }
        }
    }
}
