//! The system-prompt text of the casting chat tools (the tools are in `iyakki-agent`).

pub const PROMPT_RULE: &str = "\nCASTING (F-MEDIA-11): cast_renderers looks for TVs and speakers on the local network that can play from a media server (free; it takes a few seconds). cast_play makes one of them play an item of the person's media server (the id from the shared folders' listing, such as a3 or v1) and asks every time; the media server must be on first (Settings -> Shared folders). cast_stop stops it (asks). Nothing is sent outside the local network.";
