//! The pure address and word helpers of the media downloads (moved out of `iyakki_agent::mediadl` for F-SPLIT-03).

use crate::mediaindex::ItemKind;

pub fn ext_of_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let last = path.rsplit('/').next()?;
    let ext = last.rsplit_once('.')?.1.to_ascii_lowercase();
    (!ext.is_empty() && ext.len() <= 5).then_some(ext)
}

pub fn ext_from_type(ct: &str) -> Option<&'static str> {
    Some(match ct {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "m4a",
        "video/mp4" => "mp4",
        "video/x-matroska" => "mkv",
        "video/webm" => "webm",
        "video/x-msvideo" => "avi",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => return None,
    })
}

/// The kind of file an extension names (a picture, a video, else a song).
pub fn kind_of_ext(ext: &str) -> Option<ItemKind> {
    let e = ext.to_ascii_lowercase();
    if crate::media::EXTENSIONS.iter().any(|(x, k)| *x == e && *k == crate::media::Kind::Video) {
        Some(ItemKind::Video)
    } else if crate::mediaindex::IMAGE_EXTENSIONS.contains(&e.as_str()) {
        Some(ItemKind::Image)
    } else if crate::media::EXTENSIONS.iter().any(|(x, _)| *x == e) {
        Some(ItemKind::Song)
    } else {
        None
    }
}

/// The kind of file an address names, by its extension (None when the address says nothing).
pub fn kind_of_url(url: &str) -> Option<ItemKind> {
    kind_of_ext(&ext_of_url(url)?)
}

/// The kind a content type names (`video/webm`, `audio/mpeg`, `image/png`), if any.
pub fn kind_of_content_type(ct: &str) -> Option<ItemKind> {
    let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match ct.split('/').next()? {
        "video" => Some(ItemKind::Video),
        "image" => Some(ItemKind::Image),
        "audio" => Some(ItemKind::Song),
        _ => ext_from_type(&ct).and_then(kind_of_ext),
    }
}

/// Split what a person typed into words: commas and new lines separate, blanks around them go, empty ones go.
pub fn split_words(text: &str) -> Vec<String> {
    text.split([',', '\n', ';']).map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect()
}
