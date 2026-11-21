//! Types moved out of `iyakki_agent::imagebuildtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// The builder image this build ships, pinned by digest (`name@sha256:<64 hex>`). **The owner's to choose** after a test on a real cluster
/// (KI-851): empty means none, and `image_build` then refuses in plain words and builds nothing. An administrator may set another one for
/// this computer with `image_builder_set` (it is written to `<data>/image-builder.json` and wins over this one).
pub const BUILDER_IMAGE: &str = "";

/// True for a reference pinned by a real digest: `name@sha256:` and 64 lower-case hex digits that are not all zero.
pub fn builder_pinned(image: &str) -> bool {
    match image.rsplit_once("@sha256:") {
        Some((name, d)) => !name.is_empty() && d.len() == 64 && d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && d.bytes().any(|b| b != b'0') && !image.contains(|c: char| c.is_whitespace() || c == '"' || c == '\''),
        None => false,
    }
}

pub const PROMPT_RULE: &str = "\nIMAGE BUILDER: image_builder_status says which builder image is set up (free); image_builder_set (administrators, asks) sets or clears it and takes only an image pinned by digest, which the person must give you: never invent one. IMAGE BUILD: image_build turns a folder of the person's own files (with a Containerfile or Dockerfile) into a container image app by building it in the dev cluster; it asks first, may take minutes and pulls base images from the internet. Say what it needs: a licence for the app, the port it listens on, and an administrator account. Never invent a licence.";
