//! A picture from the clipboard history put into a text field (F-WEBDEV-01): the Editor and the Chat take the newest copy when it is a picture
//! (a picture the browser page copied, a screenshot) and keep it as a PNG file next to what is being written. HTML that came in from a page is
//! already plain text in the history, so an ordinary paste inserts it as text; only pictures need a file. Nothing is sent anywhere, no file
//! is replaced (a number is added to a taken name), and a picture over [`MAX_PNG`] bytes once encoded is refused.

use std::path::{Path, PathBuf};

/// The largest PNG kept from the clipboard.
pub const MAX_PNG: usize = 8 << 20;

/// The newest copy as a PNG, when the newest copy is a picture: `(width, height, PNG bytes)`.
pub fn newest_png() -> Result<Option<(u32, u32, Vec<u8>)>, String> {
    let Some((w, h, rgba)) = crate::ui::clipboard::newest_picture() else { return Ok(None) };
    let pm = tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h).ok_or("the picture has no size")?).ok_or("the copied picture is damaged")?;
    let png = pm.encode_png().map_err(|_| "the copied picture could not be saved as PNG".to_string())?;
    if png.len() > MAX_PNG {
        return Err(format!("the copied picture is larger than {} MB", MAX_PNG >> 20));
    }
    Ok(Some((w, h, png)))
}

/// Write `png` into `dir` as `Pasted-picture-N.png` (the first N not taken) and return the path. The file is private to the person (0600).
pub fn save(dir: &Path, png: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make the folder: {e}"))?;
    for n in 1..1000 {
        let p = dir.join(format!("Pasted-picture-{n}.png"));
        match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p) {
            Ok(mut f) => {
                f.write_all(png).map_err(|e| format!("could not save the picture: {e}"))?;
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not save the picture: {e}")),
        }
    }
    Err("too many pasted pictures in that folder".into())
}
