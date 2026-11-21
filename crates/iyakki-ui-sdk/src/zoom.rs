//! `TextZoom`: the text size an app lets the person change with a pinch and with two buttons (F-SDK-26, F-SDK-34). The size is a thousandth of
//! the person's own text size (1000 is as set in the Look), kept between [`TextZoom::MIN`] and [`TextZoom::MAX`]. A pinch reports a scale since
//! the fingers landed, so the new size is counted from the size the pinch began at, not from the last report; the end of the pinch is the moment
//! to keep the size (one storage write, not one per report). An app that builds its [`crate::size::Metrics`] with [`TextZoom::metrics`] draws every
//! widget and view at the new size.
use crate::event::Gesture;

/// What a gesture did to the zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomChange {
    /// Not a pinch (or a scale that cannot be one): nothing happened.
    None,
    /// The size moved while the fingers are on the screen.
    Moving,
    /// The fingers lifted: the size is final, keep it.
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextZoom {
    milli: u32,
    from: Option<u32>,
}

impl Default for TextZoom {
    fn default() -> TextZoom {
        TextZoom { milli: TextZoom::START, from: None }
    }
}

impl TextZoom {
    pub const MIN: u32 = 600;
    pub const MAX: u32 = 2400;
    pub const START: u32 = 1000;
    pub const STEP: u32 = 150;

    /// The size kept as text (what [`TextZoom::keep_text`] gave); anything that is not a number is the start size, a number out of range is held inside it.
    pub fn from_text(t: Option<&str>) -> TextZoom {
        let milli = t.and_then(|v| v.trim().parse::<u32>().ok()).map_or(TextZoom::START, |z| z.clamp(TextZoom::MIN, TextZoom::MAX));
        TextZoom { milli, from: None }
    }

    pub fn keep_text(&self) -> String {
        self.milli.to_string()
    }

    /// The size as a thousandth of the person's own size.
    pub fn milli(&self) -> u32 {
        self.milli
    }

    /// The factor to multiply the person's text size with.
    pub fn factor(&self) -> f32 {
        self.milli as f32 / 1000.0
    }

    /// A short name for the size, for a screen reader: "Text size 115%".
    pub fn label(&self) -> String {
        format!("Text size {}%", self.milli / 10)
    }

    pub fn set(&mut self, milli: u32) {
        self.milli = milli.clamp(TextZoom::MIN, TextZoom::MAX);
    }

    pub fn bigger(&mut self) {
        self.set(self.milli.saturating_add(TextZoom::STEP));
    }

    pub fn smaller(&mut self) {
        self.set(self.milli.saturating_sub(TextZoom::STEP));
    }

    /// A gesture: a pinch moves the size, the end of it fixes it. Every other gesture is none of this.
    pub fn gesture(&mut self, g: &Gesture) -> ZoomChange {
        match g {
            Gesture::Pinch { scale, .. } if scale.is_finite() && *scale > 0.0 => {
                let from = *self.from.get_or_insert(self.milli);
                self.set((from as f32 * scale).round().min(u32::MAX as f32) as u32);
                ZoomChange::Moving
            }
            Gesture::PinchEnd { .. } => {
                self.from = None;
                ZoomChange::Ended
            }
            _ => ZoomChange::None,
        }
    }

    /// The pane's metrics with the person's own text size and this zoom applied (what `UiCx::metrics` gives, at the zoomed size).
    #[cfg(feature = "uikit")]
    pub fn metrics(&self, cx: &crate::cx::UiCx) -> crate::size::Metrics {
        let p = cx.pane();
        crate::size::Metrics::new(p.w, p.h, cx.screen().scale, cx.text_scale * self.factor(), cx.touch).with_tokens(crate::tokens::Tokens::of(cx.look())).with_type(cx.type_style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinch(s: f32) -> Gesture {
        Gesture::Pinch { scale: s, x: 0.0, y: 0.0 }
    }

    #[test]
    fn a_pinch_counts_from_where_it_began_and_the_end_fixes_the_size() {
        let mut z = TextZoom::default();
        assert_eq!((z.milli(), z.factor()), (1000, 1.0));
        assert_eq!(z.gesture(&pinch(1.2)), ZoomChange::Moving);
        assert_eq!(z.gesture(&pinch(1.5)), ZoomChange::Moving);
        assert_eq!(z.milli(), 1500);
        assert_eq!(z.gesture(&Gesture::PinchEnd { scale: 1.5 }), ZoomChange::Ended);
        z.gesture(&pinch(0.5));
        assert_eq!(z.milli(), 750);
        z.gesture(&pinch(0.0001));
        assert_eq!(z.milli(), TextZoom::MIN);
        z.gesture(&Gesture::PinchEnd { scale: 1.0 });
        z.gesture(&pinch(1e9));
        assert_eq!(z.milli(), TextZoom::MAX);
    }

    #[test]
    fn junk_scales_and_other_gestures_change_nothing() {
        let mut z = TextZoom::default();
        for g in [pinch(f32::NAN), pinch(f32::INFINITY), pinch(-2.0), pinch(0.0), Gesture::Tap { x: 1.0, y: 1.0 }, Gesture::DragStart { x: 0.0, y: 0.0 }] {
            assert_eq!(z.gesture(&g), ZoomChange::None, "{g:?}");
        }
        assert_eq!(z.milli(), TextZoom::START);
    }

    #[test]
    fn the_buttons_step_inside_the_limits_and_the_size_is_kept_as_text() {
        let mut z = TextZoom::default();
        z.bigger();
        assert_eq!(z.milli(), 1150);
        assert_eq!(z.label(), "Text size 115%");
        for _ in 0..40 {
            z.bigger();
        }
        assert_eq!(z.milli(), TextZoom::MAX);
        for _ in 0..80 {
            z.smaller();
        }
        assert_eq!(z.milli(), TextZoom::MIN);
        z.set(1700);
        assert_eq!(TextZoom::from_text(Some(&z.keep_text())).milli(), 1700);
        assert_eq!(TextZoom::from_text(Some("99999")).milli(), TextZoom::MAX);
        assert_eq!(TextZoom::from_text(Some("1")).milli(), TextZoom::MIN);
        assert_eq!(TextZoom::from_text(Some("big")).milli(), TextZoom::START);
        assert_eq!(TextZoom::from_text(None).milli(), TextZoom::START);
    }
}
