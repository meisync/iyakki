//! The store metadata of a package (`store:` header line) and of a catalog entry: what the page of an app shows.
//! Everything here is size-capped and validated; a package whose metadata does not validate is refused.

use serde::{Deserialize, Serialize};

use crate::wtool::Cap;

pub const MAX_TITLE: usize = 60;
pub const MAX_SUMMARY: usize = 140;
pub const MAX_DESCRIPTION: usize = 8000;
pub const MAX_ICON_B64: usize = 12 * 1024;
pub const MAX_SHOT_B64: usize = 16 * 1024;

/// The tiles an extension can be for.
pub const EXT_TARGETS: [&str; 7] = ["dash", "tales", "ide", "chat", "any", "system", "flows"];
/// What an extension is.
pub const EXT_KINDS: [&str; 8] = ["connector", "template", "theme", "panel", "preset", "tool", "driver", "node"];

/// What an installed thing extends and how, for the package kinds that extend other tiles: `template` and `theme` packs, `agent`
/// presets, WASM `tool` packages and the general `extension` kind. `None` for skills, servers, command-line tools and apps.
pub fn extension_of(kind: &str, meta: &Meta) -> Option<(String, String)> {
    let target = |default: &str| if meta.target.is_empty() { default.to_string() } else { meta.target.clone() };
    Some(match kind {
        "template" => (target("dash"), "template".into()),
        "theme" => (target("any"), "theme".into()),
        "agent" => (target("chat"), "preset".into()),
        "tool" => (target("chat"), "tool".into()),
        "tile" => ("launcher".into(), "custom app".into()),
        "extension" => (target("dash"), if meta.ext_kind.is_empty() { "connector".into() } else { meta.ext_kind.clone() }),
        _ => return None,
    })
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Icon {
    /// `image/png` or `image/svg+xml`.
    pub mime: String,
    /// Base64 of the picture.
    pub data: String,
}

impl Icon {
    pub fn bytes(&self) -> Option<Vec<u8>> {
        crate::models::b64_decode(&self.data)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Publisher {
    pub id: String,
    pub name: String,
    pub url: String,
}

pub fn valid_publisher_id(id: &str) -> bool {
    (2..=32).contains(&id.len()) && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-')
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PriceModel {
    #[default]
    Free,
    Paid,
    Subscription,
    Donation,
}

impl PriceModel {
    pub fn needs_licence(self) -> bool {
        matches!(self, PriceModel::Paid | PriceModel::Subscription)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Pricing {
    pub model: PriceModel,
    /// In minor units: 499 is 4.99.
    pub price: u32,
    pub currency: String,
    pub purchase_url: String,
    pub checkout_id: String,
    pub trial_days: u32,
}

impl Pricing {
    /// "Free", "USD 4.99", "USD 2.00 / month", "Donation".
    pub fn label(&self) -> String {
        let amount = || format!("{} {}.{:02}", if self.currency.is_empty() { "USD" } else { &self.currency }, self.price / 100, self.price % 100);
        match self.model {
            PriceModel::Free => "Free".into(),
            PriceModel::Paid => amount(),
            PriceModel::Subscription => format!("{} / month", amount()),
            PriceModel::Donation => "Donation".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Dependency {
    pub name: String,
    pub min_version: String,
}

/// The descriptive part of an app.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Meta {
    pub title: String,
    pub summary: String,
    /// Markdown.
    pub description: String,
    pub icon: Option<Icon>,
    pub screenshots: Vec<Icon>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub publisher: Publisher,
    pub pricing: Pricing,
    pub permissions: Vec<String>,
    pub min_version: String,
    pub changelog: String,
    pub support_url: String,
    pub source_url: String,
    pub dependencies: Vec<Dependency>,
    /// For extensions: the tile it extends (`dash`, `tales`, `ide`, `chat`, `any`). Empty means the kind's usual one.
    pub target: String,
    /// For extensions of kind `extension`: what it is (`connector`, `template`, `theme`, `panel`, `preset`, `tool`, `driver`).
    pub ext_kind: String,
}

fn clean(s: &str, max: usize, multiline: bool, what: &str) -> Result<(), String> {
    if s.chars().count() > max {
        return Err(format!("{what} is longer than {max} characters"));
    }
    if s.chars().any(|c| c.is_control() && !(multiline && (c == '\n' || c == '\t'))) {
        return Err(format!("{what} holds control characters"));
    }
    Ok(())
}

fn url_ok(u: &str, what: &str) -> Result<(), String> {
    if u.is_empty() {
        return Ok(());
    }
    if u.len() > 300 || !(u.starts_with("https://") || u.starts_with("http://")) || u.chars().any(|c| c.is_control() || c == ' ') {
        return Err(format!("{what} must be an http or https address"));
    }
    Ok(())
}

fn picture_ok(i: &Icon, max: usize, what: &str) -> Result<(), String> {
    if i.mime != "image/png" && i.mime != "image/svg+xml" {
        return Err(format!("{what}: only PNG and SVG pictures"));
    }
    if i.data.len() > max {
        return Err(format!("{what} is larger than {} KiB", max / 1024));
    }
    let bytes = i.bytes().ok_or_else(|| format!("{what} is not valid base64"))?;
    let ok = if i.mime == "image/png" { bytes.starts_with(&[0x89, b'P', b'N', b'G']) } else { std::str::from_utf8(&bytes).is_ok_and(|t| t.contains("<svg")) };
    if !ok {
        return Err(format!("{what} is not the kind of picture it says"));
    }
    Ok(())
}

impl Meta {
    pub fn parse(json: &str) -> Result<Meta, String> {
        let m: Meta = serde_json::from_str(json).map_err(|e| format!("the store information is not readable: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn validate(&self) -> Result<(), String> {
        clean(&self.title, MAX_TITLE, false, "the title")?;
        clean(&self.summary, MAX_SUMMARY, false, "the summary")?;
        clean(&self.description, MAX_DESCRIPTION, true, "the description")?;
        clean(&self.changelog, 4000, true, "the changelog")?;
        if self.categories.len() > 5 || self.tags.len() > 8 || self.permissions.len() > 32 || self.dependencies.len() > 16 || self.screenshots.len() > 2 {
            return Err("too many categories, tags, permissions, dependencies or screenshots".into());
        }
        for w in self.categories.iter().chain(&self.tags) {
            clean(w, 24, false, "a category or tag")?;
        }
        if let Some(i) = &self.icon {
            picture_ok(i, MAX_ICON_B64, "the icon")?;
        }
        for (n, i) in self.screenshots.iter().enumerate() {
            picture_ok(i, MAX_SHOT_B64, &format!("screenshot {}", n + 1))?;
        }
        let p = &self.publisher;
        if !p.id.is_empty() && !valid_publisher_id(&p.id) {
            return Err("the publisher id: 2 to 32 lowercase letters, digits and dashes".into());
        }
        clean(&p.name, 60, false, "the publisher name")?;
        url_ok(&p.url, "the publisher address")?;
        url_ok(&self.support_url, "the support address")?;
        url_ok(&self.source_url, "the source address")?;
        url_ok(&self.pricing.purchase_url, "the purchase address")?;
        clean(&self.pricing.checkout_id, 80, false, "the checkout id")?;
        clean(&self.pricing.currency, 3, false, "the currency")?;
        clean(&self.min_version, 24, false, "the minimum version")?;
        for d in &self.dependencies {
            clean(&d.name, 48, false, "a dependency name")?;
        }
        for perm in &self.permissions {
            Permission::parse(perm)?;
        }
        if !self.target.is_empty() && !EXT_TARGETS.contains(&self.target.as_str()) {
            return Err(format!("the target must be one of {}", EXT_TARGETS.join(", ")));
        }
        if !self.ext_kind.is_empty() && !EXT_KINDS.contains(&self.ext_kind.as_str()) {
            return Err(format!("the extension kind must be one of {}", EXT_KINDS.join(", ")));
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ permissions

/// What an app asks for, in the words of the wasm-tool capabilities plus the two the store adds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Permission {
    Cap(Cap),
    /// Use a vault entry.
    Vault(String),
    /// Run a program.
    Run(String),
    /// What a tile app may ask of the bridge: `tile:storage`, `tile:clipboard`, `tile:notifications`, `tile:files`, `tile:agent`, `tile:llm`, `tile:speech`.
    Tile(String),
}

/// The bridge permissions a tile app can declare (the vault is not one of them, and never will be).
pub const TILE_PERMS: [&str; 7] = ["storage", "clipboard", "notifications", "files", "agent", "llm", "speech"];

impl Permission {
    pub fn parse(s: &str) -> Result<Permission, String> {
        if let Some(n) = s.strip_prefix("vault:") {
            if n.is_empty() || n.len() > 64 || !n.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
                return Err(format!("permission {s:?}: vault:<entry name>"));
            }
            return Ok(Permission::Vault(n.into()));
        }
        if let Some(p) = s.strip_prefix("run:") {
            if p.is_empty() || p.len() > 200 || p.chars().any(|c| c.is_control()) {
                return Err(format!("permission {s:?}: run:<program>"));
            }
            return Ok(Permission::Run(p.into()));
        }
        if let Some(t) = s.strip_prefix("tile:") {
            return if TILE_PERMS.contains(&t) { Ok(Permission::Tile(t.into())) } else { Err(format!("permission {s:?}: tile:{}", TILE_PERMS.join(" | tile:"))) };
        }
        Cap::parse(s).map(Permission::Cap)
    }

    /// Plain words, `{home}` filled in.
    pub fn words(&self, user: &str) -> String {
        match self {
            Permission::Cap(c) => c.describe(user),
            Permission::Vault(n) => format!("use the vault entry “{n}”"),
            Permission::Run(p) => format!("run the program {p}"),
            Permission::Tile(t) => match t.as_str() {
                "storage" => "keep its own saved data on this device (at most 256 KB, only this app can read it)".into(),
                "clipboard" => "read and write the clipboard".into(),
                "notifications" => "leave notes in your inbox".into(),
                "files" => "open and save files you pick in the file dialog (it never sees other files)".into(),
                "llm" => "use your AI model for its own questions, with no tools and no access to your files (you are asked once; it has a daily limit; the model is the one you chose in Settings, which may send the text to another computer)".into(),
                "speech" => "read text aloud with your voice settings (you are asked once; a voice that runs on another computer gets the text)".into(),
                _ => "ask the assistant questions, each one only after you allow it".into(),
            },
        }
    }

    /// Network, vault, run and writing files are the ones that ask the person again whatever the autonomy setting.
    pub fn sensitive(&self) -> bool {
        !matches!(self, Permission::Cap(Cap::Read(_))) && !matches!(self, Permission::Tile(t) if t != "llm" && t != "speech")
    }

    pub fn canonical(&self) -> String {
        match self {
            Permission::Cap(Cap::Read(p)) => format!("read:{p}"),
            Permission::Cap(Cap::Write(p)) => format!("write:{p}"),
            Permission::Cap(Cap::Net(h)) => format!("net:{h}"),
            Permission::Vault(n) => format!("vault:{n}"),
            Permission::Run(p) => format!("run:{p}"),
            Permission::Tile(t) => format!("tile:{t}"),
        }
    }
}

/// Permissions as sorted canonical strings (what two descriptions of the same app are compared by).
pub fn canonical_set(perms: &[String]) -> Vec<String> {
    let mut v: Vec<String> = perms.iter().map(|p| Permission::parse(p).map_or_else(|_| p.clone(), |x| x.canonical())).collect();
    v.sort();
    v.dedup();
    v
}

pub fn any_sensitive(perms: &[String]) -> bool {
    perms.iter().any(|p| Permission::parse(p).map_or(true, |x| x.sensitive()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_read_in_words_and_only_paid_models_need_a_licence() {
        let p = |model, price| Pricing { model, price, currency: "EUR".into(), ..Default::default() };
        assert_eq!(p(PriceModel::Free, 0).label(), "Free");
        assert_eq!(p(PriceModel::Paid, 499).label(), "EUR 4.99");
        assert_eq!(p(PriceModel::Subscription, 200).label(), "EUR 2.00 / month");
        assert_eq!(p(PriceModel::Donation, 0).label(), "Donation");
        assert!(PriceModel::Paid.needs_licence() && PriceModel::Subscription.needs_licence());
        assert!(!PriceModel::Free.needs_licence() && !PriceModel::Donation.needs_licence());
    }

    #[test]
    fn metadata_is_validated_and_capped() {
        let ok = Meta { title: "Hi".into(), permissions: vec!["net:example.com".into(), "vault:api".into(), "run:/bin/ls".into()], ..Default::default() };
        assert_eq!(Meta::parse(&ok.to_json()).unwrap(), ok);
        assert!(Meta { title: "x".repeat(61), ..Default::default() }.validate().is_err());
        assert!(Meta { summary: "a\nb".into(), ..Default::default() }.validate().is_err());
        assert!(Meta { description: "a\nb".into(), ..Default::default() }.validate().is_ok());
        assert!(Meta { permissions: vec!["root:everything".into()], ..Default::default() }.validate().is_err());
        assert!(Meta { support_url: "javascript:alert(1)".into(), ..Default::default() }.validate().is_err());
        let big = Icon { mime: "image/png".into(), data: "A".repeat(MAX_ICON_B64 + 1) };
        assert!(Meta { icon: Some(big), ..Default::default() }.validate().unwrap_err().contains("larger"));
        let fake = Icon { mime: "image/png".into(), data: "aGVsbG8=".into() };
        assert!(Meta { icon: Some(fake), ..Default::default() }.validate().unwrap_err().contains("kind of picture"));
        let svg = Icon { mime: "image/svg+xml".into(), data: crate::models::b64_decode("").map_or(String::new(), |_| "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=".into()) };
        assert!(Meta { icon: Some(svg), ..Default::default() }.validate().is_ok());
        assert!(Meta { publisher: Publisher { id: "A".into(), ..Default::default() }, ..Default::default() }.validate().is_err());
        assert!(Meta::parse("{not json").is_err());
    }

    #[test]
    fn extensions_are_mapped_from_the_package_kinds_and_checked() {
        let m = Meta::default();
        assert_eq!(extension_of("template", &m), Some(("dash".into(), "template".into())));
        assert_eq!(extension_of("theme", &m), Some(("any".into(), "theme".into())));
        assert_eq!(extension_of("agent", &m), Some(("chat".into(), "preset".into())));
        assert_eq!(extension_of("tool", &m), Some(("chat".into(), "tool".into())));
        let tales = Meta { target: "tales".into(), ..Default::default() };
        assert_eq!(extension_of("theme", &tales), Some(("tales".into(), "theme".into())));
        let panel = Meta { target: "dash".into(), ext_kind: "panel".into(), ..Default::default() };
        assert_eq!(extension_of("extension", &panel), Some(("dash".into(), "panel".into())));
        assert_eq!(extension_of("extension", &m), Some(("dash".into(), "connector".into())));
        for k in ["skill", "mcp", "cli", "app"] {
            assert!(extension_of(k, &m).is_none(), "{k}");
        }
        assert!(Meta { target: "everything".into(), ..Default::default() }.validate().is_err());
        assert!(Meta { ext_kind: "virus".into(), ..Default::default() }.validate().is_err());
        assert!(panel.validate().is_ok());
    }

    #[test]
    fn permissions_are_said_in_plain_words_and_the_risky_ones_are_marked() {
        let w = |s: &str| Permission::parse(s).unwrap().words("alice");
        assert_eq!(w("read:{home}/notes"), "read files in /home/alice/notes");
        assert_eq!(w("net:Example.com"), "connect to example.com");
        assert_eq!(w("vault:api-key"), "use the vault entry “api-key”");
        assert_eq!(w("run:/usr/bin/foo"), "run the program /usr/bin/foo");
        assert!(!any_sensitive(&["read:/shared".into()]));
        for p in ["net:a.com", "write:/tmp", "vault:x", "run:/bin/ls"] {
            assert!(any_sensitive(&[p.into()]), "{p}");
        }
        assert_eq!(canonical_set(&["net:B.com".into(), "net:b.com".into(), "read:/a".into()]), vec!["net:b.com".to_string(), "read:/a".to_string()]);
    }
}
