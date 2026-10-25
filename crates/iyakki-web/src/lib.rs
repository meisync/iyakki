//! A small web engine for reading: no scripts, no styles, no plugins. A page becomes a list of blocks (headings,
//! paragraphs, list items, preformatted text, rules, tables) made of styled spans, some of which are links.
//!
//! * [`url`] parses and joins web addresses (only `http` and `https`, never `javascript:` or `file:`).
//! * [`html`] is a forgiving parser that never fails and never recurses without bound.
//! * [`doc`] turns the tree into blocks.
//! * [`fetch`] loads a page through any transport, follows redirects and checks every hop against a host policy.

pub mod blocker;
pub mod blocker_json;
pub mod blocker_lists;
pub mod blocker_state;
pub mod cache;
pub mod cookies;
pub mod css;
pub mod doc;
pub mod domtree;
pub mod fetch;
pub mod gif;
pub mod html;
pub mod image;
pub mod jpeg;
pub mod media;
pub mod net;
pub mod preload;
pub mod netstats;
pub mod reader;
pub mod style;
pub mod svg;
pub mod url;
pub mod webp;

pub use cookies::{Cookie, CookieStore};
pub use doc::{Block, Control, ControlKind, Doc, Form, ImageRef, SelectOption, Span, Style, TextBlock};
pub use fetch::{fetch_following, load, load_request, load_with, Fetch, Jar, Loaded, Renderer, Request, Response};
pub use url::{form_urlencode, SearchEngine, Url};
