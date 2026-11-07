//! A person's own ratings of packages: 1 to 5 stars and a short private comment, kept for that account only
//! (`<store>/ratings-<user>.json`, schema-stamped, written atomically with the shared helpers of `iyakki_schema`).
//!
//! There is deliberately no network code here and none in the tile's use of it: a rating is a note to yourself. Nothing reads these
//! files except the STORE tile for the signed-in account, no catalog or store server is told about them, and the export of the
//! person's arrangement (organize.rs) does not include them. Sharing ratings between people is a design question (identity, abuse,
//! moderation, privacy) answered in docs/STORE.md ("Ratings (after beta)").
//!
//! Items are named by `kind:name` like the arrangement (see [`super::organize::item_key`]), NOT by version, so a rating stays when the
//! package updates, rolls back, or is removed and installed again. Only "forget my rating" ([`Ratings::forget`]) deletes it.
//! A comment is refused when it holds a credential (`credguard::detect`): the file is private, but a password has no place in a note.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The longest comment, in characters.
pub const MAX_COMMENT: usize = 500;
/// The most packages one account can keep a rating for (a damaged or hand-made file is cut to this).
pub const MAX_RATINGS: usize = 2000;
/// The largest ratings file that is read.
pub const MAX_FILE: usize = 2 * 1024 * 1024;

/// `<store>/ratings-<user>.json`. Schema 1 is the first release of this file.
pub const SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "package ratings", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "stamp the first layout", run: |_| {} }] };

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Rating {
    /// 0 (a comment without stars) to 5.
    pub stars: u8,
    pub comment: String,
    /// The version that was on the page when the person last changed this (information only).
    pub version: String,
    /// Unix time of the last change.
    pub at: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct RatingsFile {
    schema: u32,
    ratings: BTreeMap<String, Rating>,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

pub struct Ratings {
    path: PathBuf,
    file: RatingsFile,
}

fn clean_comment(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Ratings {
    pub fn path_of(dir: &Path, user: &str) -> PathBuf {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        dir.join(format!("ratings-{safe}.json"))
    }

    /// No ratings and nowhere to save them (what the tile holds until it has read the account's file).
    pub fn none() -> Ratings {
        Ratings { path: PathBuf::new(), file: RatingsFile::default() }
    }

    /// The account's ratings (empty when there is no file; a damaged file is moved aside and the account starts empty).
    pub fn load(dir: &Path, user: &str) -> Ratings {
        let path = Ratings::path_of(dir, user);
        let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() as usize > MAX_FILE);
        if too_big {
            iyakki_schema::move_aside(&path, iyakki_schema::now_secs());
        }
        let (_, mut file, _) = iyakki_schema::load_typed::<RatingsFile>(&SCHEMA, &path, iyakki_schema::now_secs());
        // drop what a hand-edited file could not have been made by the tile
        file.ratings.retain(|k, r| !k.is_empty() && k.len() <= 200 && (r.stars <= 5));
        while file.ratings.len() > MAX_RATINGS {
            file.ratings.pop_last();
        }
        for r in file.ratings.values_mut() {
            r.comment = clean_comment(&r.comment).chars().take(MAX_COMMENT).collect();
        }
        file.ratings.retain(|_, r| r.stars > 0 || !r.comment.is_empty());
        Ratings { path, file }
    }

    pub fn get(&self, key: &str) -> Option<&Rating> {
        self.file.ratings.get(key)
    }

    pub fn len(&self) -> usize {
        self.file.ratings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.file.ratings.is_empty()
    }

    fn commit(&mut self, next: BTreeMap<String, Rating>) -> Result<(), String> {
        let file = RatingsFile { schema: SCHEMA.current, ratings: next, extra: self.file.extra.clone() };
        iyakki_schema::save_typed(&SCHEMA, &self.path, &file, 0o600)?;
        self.file = file;
        Ok(())
    }

    fn change(&mut self, key: &str, version: &str, f: impl FnOnce(&mut Rating)) -> Result<(), String> {
        let mut next = self.file.ratings.clone();
        if !next.contains_key(key) && next.len() >= MAX_RATINGS {
            return Err(format!("You can keep ratings for at most {MAX_RATINGS} packages."));
        }
        let r = next.entry(key.to_string()).or_default();
        f(r);
        r.version = version.chars().filter(|c| !c.is_control()).take(40).collect();
        r.at = iyakki_schema::now_secs();
        if r.stars == 0 && r.comment.is_empty() {
            next.remove(key);
        }
        self.commit(next)
    }

    /// Set the stars (1 to 5; 0 takes the stars away and keeps the comment). Replaces the earlier stars.
    pub fn set_stars(&mut self, key: &str, stars: u8, version: &str) -> Result<(), String> {
        if stars > 5 {
            return Err("A rating is 1 to 5 stars.".into());
        }
        self.change(key, version, |r| r.stars = stars)
    }

    /// Set the comment (an empty text takes it away and keeps the stars). Replaces the earlier comment.
    pub fn set_comment(&mut self, key: &str, text: &str, version: &str) -> Result<(), String> {
        let text = clean_comment(text);
        if text.chars().count() > MAX_COMMENT {
            return Err(format!("A comment is at most {MAX_COMMENT} characters; this one has {}.", text.chars().count()));
        }
        if let Some(f) = crate::credguard::detect(&text) {
            return Err(format!("That comment looks like it holds a credential ({}). Take it out and try again: comments are for your opinion, never for passwords or keys.", f.what));
        }
        self.change(key, version, |r| r.comment = text)
    }

    /// Forget the stars and the comment for one package. `false` when there was nothing to forget.
    pub fn forget(&mut self, key: &str) -> Result<bool, String> {
        if !self.file.ratings.contains_key(key) {
            return Ok(false);
        }
        let mut next = self.file.ratings.clone();
        next.remove(key);
        self.commit(next).map(|_| true)
    }
}

#[cfg(test)]
#[path = "ratings_tests.rs"]
mod tests;
