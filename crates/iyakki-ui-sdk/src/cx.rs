//! The context an app gets on every call: the Look, the screen, the pane, the view, the size class, and the dock and wallpaper ports.
//! Nothing here reaches the machine: an app that needs more gets it through a port of its own that the host hands it.

use iyakki_uikit::look::Look;
use iyakki_uikit::skins::{self, Accent, SkinInfo};
use iyakki_uikit::theme::Theme;
use iyakki_uikit::wallpaper;

use crate::desktop::{Arrangement, DockPort, Pane, Screen, ViewInfo, WallpaperPort};
use crate::size::{Metrics, SizeClass};
use crate::tokens::Tokens;
use crate::typeface::TypeStyle;

pub struct UiCx<'a> {
    pub look: &'a Look,
    pub arrangement: &'a Arrangement,
    pub pane: &'a Pane,
    pub view: &'a ViewInfo,
    pub touch: bool,
    pub text_scale: f32,
    /// The host's clock at the last heartbeat, in milliseconds (0 before the first): for pacing, never for the date.
    pub now_ms: u64,
    pub type_style: TypeStyle,
    pub dock: &'a mut dyn DockPort,
    pub wallpaper: &'a mut dyn WallpaperPort,
}

impl UiCx<'_> {
    /// The colours: always draw with these, never with a colour of your own (a lint test scans SDK apps for literals).
    pub fn colors(&self) -> &Theme {
        &self.look.theme
    }

    pub fn look(&self) -> &Look {
        self.look
    }

    /// The person's type choices: family, line height, letter spacing, tabular figures. Ask for a role (`FontRole`), never a file.
    pub fn fonts(&self) -> TypeStyle {
        self.type_style
    }

    /// The skin in use: its key and name, whether it is dark, glass, reduced motion, and its shape, edge and spacing words. The person
    /// can switch it at any time; the next `view` call sees the new one (nothing restarts).
    pub fn skin(&self) -> SkinInfo {
        SkinInfo::of(self.look)
    }

    /// The accent an app wants to ship, after the contrast check (WCAG AA: the label on it 4.5:1, it against the card and the page 3:1).
    /// An accent that fails is moved towards the skin's own, or replaced by it; `adjusted` says so.
    pub fn accent(&self, wanted: [u8; 4]) -> Accent {
        skins::fit_accent(&self.look.theme, wanted)
    }

    /// The skin's colours with the app's accent fitted (a copy): draw with it and the widgets use the app's accent wherever it reads.
    pub fn colors_with_accent(&self, wanted: [u8; 4]) -> Theme {
        Theme { accent: self.accent(wanted).colour, ..self.look.theme }
    }

    /// The screen this pane is on (the primary one when its own was unplugged).
    pub fn screen(&self) -> &Screen {
        self.arrangement.resolve(self.pane.screen)
    }

    pub fn pane(&self) -> &Pane {
        self.pane
    }

    pub fn view(&self) -> &ViewInfo {
        self.view
    }

    pub fn size_class(&self) -> SizeClass {
        SizeClass::for_width(self.pane.w)
    }

    pub fn metrics(&self) -> Metrics {
        Metrics::new(self.pane.w, self.pane.h, self.screen().scale, self.text_scale, self.touch).with_tokens(Tokens::of(self.look)).with_type(self.type_style)
    }

    /// The skin's numbers beyond colour (radii, borders, gaps, shadow, glass, glyph size, motion): read these, never write a number.
    pub fn tokens(&self) -> Tokens {
        Tokens::of(self.look)
    }

    /// The wallpaper of this pane's screen.
    pub fn wallpaper_here(&self) -> wallpaper::Id {
        self.arrangement.wallpaper_for(self.look, self.pane.screen)
    }

    /// The colour for text drawn straight on this screen's wallpaper.
    pub fn text_on_wallpaper(&self) -> [u8; 4] {
        crate::desktop::text_over_wallpaper(self.look, self.wallpaper_here())
    }
}
