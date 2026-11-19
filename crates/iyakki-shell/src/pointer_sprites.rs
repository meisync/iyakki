//! Mouse pointer sprites for the bare-metal framebuffer, drawn in software: small ASCII-art bitmaps
//! ('X' black, '.' white, ' ' transparent), one per [`crate::ui::Pointer`] shape, no assets.
//! Each sprite has a hotspot: the sprite pixel that sits exactly on the click point.

use crate::ui::Pointer as Shape;

/// A sprite: its rows, its size in sprite pixels and its hotspot (sprite pixel on the pointer position).
pub struct Sprite {
    pub rows: &'static [&'static str],
    pub w: i32,
    pub h: i32,
    pub hot: (i32, i32),
}

const ARROW: &[&str] = &[
    "X",
    "XX",
    "X.X",
    "X..X",
    "X...X",
    "X....X",
    "X.....X",
    "X......X",
    "X.......X",
    "X........X",
    "X.....XXXXX",
    "X..X..X",
    "X.X X..X",
    "XX  X..X",
    "X    X..X",
    "     X..X",
    "      XX",
];

const HAND: &[&str] = &[
    "   XXXX",
    "   X..X",
    "   X..X",
    "   X..X",
    "   X..XXXX",
    "   X..X..XXXX",
    "XXXX..X..X..XXXX",
    "X..X..X..X..X..X",
    "X..............X",
    "X..............X",
    "XX.............X",
    " XX............X",
    "  X............X",
    "  XX...........X",
    "   X..........XX",
    "   XX........XX",
    "    X........X",
    "    XXXXXXXXXX",
];

const TEXT: &[&str] = &[
    "XXXXXXX",
    "X.....X",
    "XXX.XXX",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "  X.X",
    "XXX.XXX",
    "X.....X",
    "XXXXXXX",
];

const WAIT: &[&str] = &[
    "XXXXXXXXXXX",
    "X.........X",
    " X.......X",
    "  X.....X",
    "   X...X",
    "    X.X",
    "    X.X",
    "   X...X",
    "  X.....X",
    " X.......X",
    "X.........X",
    "XXXXXXXXXXX",
];

const CROSS: &[&str] = &[
    "      XXX",
    "      X.X",
    "      X.X",
    "      X.X",
    "      X.X",
    "      XXX",
    "XXXXXX   XXXXXX",
    "X....X X X....X",
    "XXXXXX   XXXXXX",
    "      XXX",
    "      X.X",
    "      X.X",
    "      X.X",
    "      X.X",
    "      XXX",
];

const MOVE: &[&str] = &[
    "       XXX",
    "      XX.XX",
    "     XX...XX",
    "    XX.....XX",
    "   XX.......XX",
    "  XX.XXX.XXX.XX",
    " XX..XXX.XXX..XX",
    "XX...XX...XX...XX",
    "X...............X",
    "XX...XX...XX...XX",
    " XX..XXX.XXX..XX",
    "  XX.XXX.XXX.XX",
    "   XX.......XX",
    "    XX.....XX",
    "     XX...XX",
    "      XX.XX",
    "       XXX",
];

const NOT_ALLOWED: &[&str] = &[
    "        XXX",
    "     XXXX.XXXX",
    "   XXX.......XXX",
    "  XX...........XX",
    "  X...XXXXXXX...X",
    " XX....XX   XX..XX",
    " X..X...XX   XX..X",
    " X..XX...XX   X..X",
    "XX..XXX...XX  X..XX",
    "X...X XX...XX X...X",
    "XX..X  XX...XXX..XX",
    " X..X   XX...XX..X",
    " X..XX   XX...X..X",
    " XX..XX   XX....XX",
    "  X...XXXXXXX...X",
    "  XX...........XX",
    "   XXX.......XXX",
    "     XXXX.XXXX",
    "        XXX",
];

const RESIZE_H: &[&str] = &[
    "   XXX       XXX",
    "  XX.X       X.XX",
    " XX..X       X..XX",
    "XX...XXXXXXXXX...XX",
    "X.................X",
    "XX...XXXXXXXXX...XX",
    " XX..X       X..XX",
    "  XX.X       X.XX",
    "   XXX       XXX",
];

const RESIZE_V: &[&str] = &[
    "   XXX",
    "  XX.XX",
    " XX...XX",
    "XX.....XX",
    "X.......X",
    "XXXX.XXXX",
    "   X.X",
    "   X.X",
    "   X.X",
    "   X.X",
    "   X.X",
    "   X.X",
    "   X.X",
    "XXXX.XXXX",
    "X.......X",
    "XX.....XX",
    " XX...XX",
    "  XX.XX",
    "   XXX",
];

const GRAB: &[&str] = &[
    "  XXXXXXXXXX",
    " XX..X..X..XXXX",
    "XX............XX",
    "X..............X",
    "X..............X",
    "XX............XX",
    " X............X",
    " XX...........X",
    "  XX..........X",
    "   XX........XX",
    "    XXXXXXXXXX",
];

const HELP: &[&str] = &[
    "X",
    "XX",
    "X.X",
    "X..X",
    "X...X       XXXXX",
    "X....X     XX...XX",
    "X.....X    X.XXX.X",
    "X......X   XXXXX.X",
    "X.......X    XX.XX",
    "X........X   X.XX",
    "X.....XXXXX  X.X",
    "X..X..X      XXX",
    "X.X X..X     X.X",
    "XX  X..X     XXX",
    "X    X..X",
    "     X..X",
    "      XX",
];

pub static S_ARROW: Sprite = Sprite { rows: ARROW, w: 12, h: 17, hot: (0, 0) };
pub static S_HAND: Sprite = Sprite { rows: HAND, w: 16, h: 18, hot: (4, 0) };
pub static S_TEXT: Sprite = Sprite { rows: TEXT, w: 7, h: 17, hot: (3, 8) };
pub static S_WAIT: Sprite = Sprite { rows: WAIT, w: 11, h: 12, hot: (5, 6) };
pub static S_CROSS: Sprite = Sprite { rows: CROSS, w: 15, h: 15, hot: (7, 7) };
pub static S_MOVE: Sprite = Sprite { rows: MOVE, w: 17, h: 17, hot: (8, 8) };
pub static S_NOT_ALLOWED: Sprite = Sprite { rows: NOT_ALLOWED, w: 19, h: 19, hot: (9, 9) };
pub static S_RESIZE_H: Sprite = Sprite { rows: RESIZE_H, w: 19, h: 9, hot: (9, 4) };
pub static S_RESIZE_V: Sprite = Sprite { rows: RESIZE_V, w: 9, h: 19, hot: (4, 9) };
pub static S_GRAB: Sprite = Sprite { rows: GRAB, w: 16, h: 11, hot: (8, 5) };
pub static S_HELP: Sprite = Sprite { rows: HELP, w: 18, h: 17, hot: (0, 0) };

/// The sprite for a pointer shape; `None` for `Hidden` (nothing is drawn).
pub fn sprite(shape: Shape) -> Option<&'static Sprite> {
    Some(match shape {
        Shape::Default => &S_ARROW,
        Shape::Hand => &S_HAND,
        Shape::Text => &S_TEXT,
        Shape::Wait => &S_WAIT,
        Shape::Crosshair => &S_CROSS,
        Shape::Move => &S_MOVE,
        Shape::NotAllowed => &S_NOT_ALLOWED,
        Shape::ResizeH => &S_RESIZE_H,
        Shape::ResizeV => &S_RESIZE_V,
        Shape::Grab => &S_GRAB,
        Shape::Help => &S_HELP,
        Shape::Hidden => return None,
    })
}

/// Every shape, in the order of [`shape_to_u8`].
pub const ALL: [Shape; 12] = [
    Shape::Default,
    Shape::Hand,
    Shape::Text,
    Shape::Wait,
    Shape::Crosshair,
    Shape::Move,
    Shape::NotAllowed,
    Shape::ResizeH,
    Shape::ResizeV,
    Shape::Grab,
    Shape::Help,
    Shape::Hidden,
];

/// A shape as a byte for the atomic shared between the threads.
pub fn shape_to_u8(shape: Shape) -> u8 {
    match shape {
        Shape::Default => 0,
        Shape::Hand => 1,
        Shape::Text => 2,
        Shape::Wait => 3,
        Shape::Crosshair => 4,
        Shape::Move => 5,
        Shape::NotAllowed => 6,
        Shape::ResizeH => 7,
        Shape::ResizeV => 8,
        Shape::Grab => 9,
        Shape::Help => 10,
        Shape::Hidden => 11,
    }
}

/// The shape for a byte; anything unknown is the arrow.
pub fn shape_from_u8(b: u8) -> Shape {
    ALL.get(b as usize).copied().unwrap_or(Shape::Default)
}
