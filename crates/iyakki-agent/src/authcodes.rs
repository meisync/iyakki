//! The AUTH tile's data: one-time-code entries (issuer, account, secret, hash, digits, step, group) and the groups they sit in. All
//! of it, secrets and names alike, is ONE secret in the person's own vault (`auth.codes`, JSON), so nothing about the accounts is
//! on disk in the clear and the vault's password protects it all. The vault is unlocked by the person's sign-in password; when it
//! is locked the tile says so and shows nothing.
//!
//! [`Book`] is the pure model (add, edit, move, delete, groups, the code now); [`CodeVault`] is where it is kept: the real one is
//! [`ServicesVault`], tests use a fake. Nothing here logs and no message contains a secret or a code. The secrets are zeroized when
//! an entry is dropped.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::totp::{self, Algo, MAX_NAME_CHARS};

/// The name of the vault secret that holds the whole book.
pub const VAULT_NAME: &str = "auth.codes";
/// Entries and groups a book may hold (the vault keeps 64 KiB per secret).
pub const MAX_ENTRIES: usize = 200;
pub const MAX_GROUPS: usize = 30;
/// The group a new book starts with.
pub const DEFAULT_GROUP: &str = "Personal";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub issuer: String,
    pub account: String,
    /// Normalised base32.
    pub secret: String,
    /// `SHA1`, `SHA256` or `SHA512`.
    pub algo: String,
    pub digits: u32,
    pub period: u32,
    pub group: String,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry").field("id", &self.id).field("issuer", &self.issuer).field("account", &self.account).field("secret", &"<hidden>").field("group", &self.group).finish()
    }
}

impl Entry {
    pub fn algorithm(&self) -> Algo {
        Algo::parse(&self.algo).unwrap_or(Algo::Sha1)
    }

    /// `Issuer (account)`, or whichever of the two there is.
    pub fn title(&self) -> String {
        match (self.issuer.is_empty(), self.account.is_empty()) {
            (false, false) => format!("{} ({})", self.issuer, self.account),
            (false, true) => self.issuer.clone(),
            _ => self.account.clone(),
        }
    }

    /// The code at a Unix time, or `None` when the stored secret cannot be read (a damaged entry).
    pub fn code_at(&self, unix: i64) -> Option<String> {
        let key = totp::base32_decode(&self.secret).ok()?;
        Some(totp::totp(self.algorithm(), &key, unix, self.period, self.digits))
    }
}

/// What the person typed (or a link or a QR code said) for a new entry.
#[derive(Clone)]
pub struct NewEntry {
    pub issuer: String,
    pub account: String,
    /// Base32 as typed (any spacing and case).
    pub secret: Zeroizing<String>,
    pub algo: Algo,
    pub digits: u32,
    pub period: u32,
    pub group: String,
}

impl std::fmt::Debug for NewEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewEntry").field("issuer", &self.issuer).field("account", &self.account).field("secret", &"<hidden>").finish()
    }
}

impl NewEntry {
    /// An entry from typed text.
    pub fn typed(issuer: &str, account: &str, secret: &str, (algo, digits, period): (Algo, u32, u32), group: &str) -> NewEntry {
        NewEntry { issuer: issuer.into(), account: account.into(), secret: Zeroizing::new(secret.into()), algo, digits, period, group: group.into() }
    }

    pub fn from_link(p: totp::Parsed, group: &str) -> NewEntry {
        NewEntry { issuer: p.issuer, account: p.account, secret: p.secret, algo: p.algo, digits: p.digits, period: p.period, group: group.to_string() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Book {
    /// Format version.
    pub v: u32,
    pub groups: Vec<String>,
    pub entries: Vec<Entry>,
    next: u64,
}

impl Default for Book {
    fn default() -> Book {
        Book { v: 1, groups: vec![DEFAULT_GROUP.to_string()], entries: Vec::new(), next: 1 }
    }
}

/// A name for an issuer, an account or a group: trimmed, not empty when `need`, short, no control characters.
fn clean(what: &str, s: &str, need: bool) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() && need {
        return Err(format!("Type a {what} first."));
    }
    if t.chars().count() > MAX_NAME_CHARS || t.chars().any(char::is_control) {
        return Err(format!("The {what} is too long (at most {MAX_NAME_CHARS} characters) or has odd characters."));
    }
    Ok(t.to_string())
}

impl Book {
    /// A book from its stored text.
    pub fn parse(text: &str) -> Result<Book, String> {
        let mut b: Book = serde_json::from_str(text).map_err(|_| "Your saved one-time codes could not be read.".to_string())?;
        if b.v != 1 {
            return Err("Your saved one-time codes are from a newer version.".into());
        }
        if b.groups.is_empty() {
            b.groups.push(DEFAULT_GROUP.to_string());
        }
        // an entry whose group is gone goes to the first group
        let first = b.groups[0].clone();
        let known = b.groups.clone();
        for e in &mut b.entries {
            if !known.contains(&e.group) {
                e.group = first.clone();
            }
        }
        let top = b.entries.iter().map(|e| e.id).max().unwrap_or(0);
        b.next = b.next.max(top + 1);
        Ok(b)
    }

    /// The text kept in the vault.
    pub fn to_text(&self) -> Result<Zeroizing<String>, String> {
        let t = Zeroizing::new(serde_json::to_string(self).map_err(|_| "The codes could not be saved.".to_string())?);
        if t.len() > crate::vault::MAX_SECRET_BYTES {
            return Err("There are too many codes to keep in the vault.".into());
        }
        Ok(t)
    }

    pub fn in_group<'a>(&'a self, group: &'a str) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries.iter().filter(move |e| e.group == group)
    }

    pub fn count_in(&self, group: &str) -> usize {
        self.in_group(group).count()
    }

    pub fn entry(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn group_name(&self, name: &str) -> Result<String, String> {
        let n = clean("group name", name, true)?;
        self.groups.iter().find(|g| g.eq_ignore_ascii_case(&n)).cloned().ok_or_else(|| "That group does not exist.".to_string())
    }

    pub fn add_group(&mut self, name: &str) -> Result<String, String> {
        let n = clean("group name", name, true)?;
        if self.groups.iter().any(|g| g.to_lowercase() == n.to_lowercase()) {
            return Err("There is already a group with that name.".into());
        }
        if self.groups.len() >= MAX_GROUPS {
            return Err(format!("At most {MAX_GROUPS} groups."));
        }
        self.groups.push(n.clone());
        Ok(n)
    }

    pub fn rename_group(&mut self, old: &str, new: &str) -> Result<String, String> {
        let old = self.group_name(old)?;
        let n = clean("group name", new, true)?;
        if self.groups.iter().any(|g| *g != old && g.to_lowercase() == n.to_lowercase()) {
            return Err("There is already a group with that name.".into());
        }
        for g in &mut self.groups {
            if *g == old {
                *g = n.clone();
            }
        }
        for e in &mut self.entries {
            if e.group == old {
                e.group = n.clone();
            }
        }
        Ok(n)
    }

    /// Delete a group; its codes are kept and move to the first group left. The last group cannot be deleted.
    /// Returns how many codes moved.
    pub fn delete_group(&mut self, name: &str) -> Result<usize, String> {
        let name = self.group_name(name)?;
        if self.groups.len() < 2 {
            return Err("This is your only group. Make another one first.".into());
        }
        self.groups.retain(|g| *g != name);
        let to = self.groups[0].clone();
        let mut moved = 0;
        for e in &mut self.entries {
            if e.group == name {
                e.group = to.clone();
                moved += 1;
            }
        }
        Ok(moved)
    }

    /// Add an entry; returns its id.
    pub fn add(&mut self, n: &NewEntry) -> Result<u64, String> {
        if self.entries.len() >= MAX_ENTRIES {
            return Err(format!("At most {MAX_ENTRIES} codes."));
        }
        let issuer = clean("service name", &n.issuer, false)?;
        let account = clean("account name", &n.account, false)?;
        if issuer.is_empty() && account.is_empty() {
            return Err("Type the service name or the account name.".into());
        }
        if !(6..=8).contains(&n.digits) {
            return Err("A code has 6, 7 or 8 digits.".into());
        }
        if !(1..=3600).contains(&n.period) {
            return Err("The time step is between 1 and 3600 seconds.".into());
        }
        let secret = totp::normalize_secret(&n.secret)?;
        let group = self.group_name(&n.group)?;
        if self.entries.iter().any(|e| e.issuer == issuer && e.account == account && e.secret == *secret) {
            return Err("That code is already in your list.".into());
        }
        let id = self.next;
        self.next += 1;
        self.entries.push(Entry { id, issuer, account, secret: secret.to_string(), algo: n.algo.name().into(), digits: n.digits, period: n.period, group });
        Ok(id)
    }

    /// Change the names of an entry (the secret is never edited: delete and add again).
    pub fn edit(&mut self, id: u64, issuer: &str, account: &str) -> Result<(), String> {
        let (issuer, account) = (clean("service name", issuer, false)?, clean("account name", account, false)?);
        if issuer.is_empty() && account.is_empty() {
            return Err("Type the service name or the account name.".into());
        }
        let e = self.entries.iter_mut().find(|e| e.id == id).ok_or("That code is not in the list any more.")?;
        e.issuer = issuer;
        e.account = account;
        Ok(())
    }

    pub fn move_to(&mut self, id: u64, group: &str) -> Result<(), String> {
        let g = self.group_name(group)?;
        let e = self.entries.iter_mut().find(|e| e.id == id).ok_or("That code is not in the list any more.")?;
        e.group = g;
        Ok(())
    }

    pub fn delete(&mut self, id: u64) -> Result<Entry, String> {
        let i = self.entries.iter().position(|e| e.id == id).ok_or("That code is not in the list any more.")?;
        Ok(self.entries.remove(i))
    }
}

/// Why the vault could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultProblem {
    /// The person's vault is locked (it could not be opened at sign-in).
    Locked,
    /// Anything else, in plain words (never a secret).
    Other(String),
}

/// Where the book is kept.
pub trait CodeVault {
    fn load(&self) -> Result<Book, VaultProblem>;
    fn save(&self, book: &Book) -> Result<(), VaultProblem>;
}

/// The real place: the signed-in person's own vault.
pub struct ServicesVault {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl CodeVault for ServicesVault {
    fn load(&self) -> Result<Book, VaultProblem> {
        if !self.svc.user_vault_unlocked(&self.user) {
            return Err(VaultProblem::Locked);
        }
        match self.svc.user_get(&self.user, VAULT_NAME) {
            Ok(t) => Book::parse(&t).map_err(VaultProblem::Other),
            Err(crate::vault::VaultError::NotFound(_)) => Ok(Book::default()),
            Err(crate::vault::VaultError::Locked) => Err(VaultProblem::Locked),
            Err(e) => Err(VaultProblem::Other(e.to_string())),
        }
    }

    fn save(&self, book: &Book) -> Result<(), VaultProblem> {
        let text = book.to_text().map_err(VaultProblem::Other)?;
        self.svc.user_set(&self.user, VAULT_NAME, &text).map_err(|e| match e {
            crate::vault::VaultError::Locked => VaultProblem::Locked,
            other => VaultProblem::Other(other.to_string()),
        })
    }
}

/// True when the clock is plausibly set: not before the floor every image is newer than (an unset clock reads 1970 or the date of the
/// last shutdown), so a code made from it would be wrong.
pub fn clock_is_set(unix: i64) -> bool {
    unix >= crate::clockfloor::COMPILED_FLOOR
}

#[cfg(test)]
#[path = "authcodes_tests.rs"]
mod tests;
