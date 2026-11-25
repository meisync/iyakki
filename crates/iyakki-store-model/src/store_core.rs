use crate::store_catalog::Entry;
use crate::store_catalog::Loaded;
use crate::store_trust::Level;
use crate::store_trust::Verdict;

/// What a private store's catalog says about itself, shown before it is trusted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Probe {
    pub name: String,
    pub id: String,
    pub key: String,
    pub fingerprint: String,
    pub serial: u64,
    pub entries: usize,
    pub url: String,
    /// Who signed it and how far that is trusted, in words (shown before the store is enabled).
    pub trust: String,
}

/// What a look at the network found.
#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct Refresh {
    pub loaded: Vec<Loaded>,
    pub errors: Vec<(String, String)>,
    pub notes: Vec<String>,
    /// Apps turned off because they were withdrawn.
    pub disabled: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum State {
    Available,
    Installed,
    Update(String),
    Disabled,
    /// Paid, and no licence on this device.
    Locked,
    /// A made-up example.
    Demo,
    /// Opens another tile to install.
    Link(String),
    Revoked,
}

impl State {
    pub fn label(&self) -> String {
        match self {
            State::Available => "Available".into(),
            State::Installed => "Installed".into(),
            State::Update(v) => format!("Update {v}"),
            State::Disabled => "Turned off".into(),
            State::Locked => "Locked".into(),
            State::Demo => "DEMO".into(),
            State::Link(t) => format!("Opens {}", t.to_uppercase()),
            State::Revoked => "Revoked".into(),
        }
    }

    pub fn is_installed(&self) -> bool {
        matches!(self, State::Installed | State::Update(_) | State::Disabled)
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Item {
    pub entry: Entry,
    pub source: String,
    pub source_name: String,
    pub verdict: Verdict,
    pub state: State,
    pub installed_version: Option<String>,
    pub preinstalled: bool,
    /// False when the catalog has expired.
    pub installable: bool,
}

impl Item {
    pub fn title(&self) -> &str {
        self.entry.title()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PriceFilter {
    #[default]
    All,
    Free,
    Paid,
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub query: String,
    pub price: PriceFilter,
    pub level: Option<Level>,
    pub kind: Option<String>,
    pub category: Option<String>,
    pub publisher: Option<String>,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use crate::store_manifest::PriceModel;

pub fn score(i: &Item, words: &[String]) -> Option<u32> {
    if words.is_empty() {
        return Some(0);
    }
    let e = &i.entry;
    let (title, name, summary) = (e.title().to_lowercase(), e.name.to_lowercase(), e.meta.summary.to_lowercase());
    let tags = e.meta.tags.iter().chain(&e.meta.categories).map(|t| t.to_lowercase()).collect::<Vec<_>>();
    let publisher = e.meta.publisher.name.to_lowercase();
    let mut total = 0;
    for w in words {
        let s = if title == *w || name == *w { 10 } else if title.contains(w.as_str()) || name.contains(w.as_str()) { 6 } else if tags.iter().any(|t| t.contains(w.as_str())) { 4 } else if summary.contains(w.as_str()) || publisher.contains(w.as_str()) || e.kind == *w { 2 } else { return None };
        total += s;
    }
    Some(total)
}

/// The items that pass the filters, best matches first (then featured, then by title).
pub fn filter<'a>(items: &'a [Item], f: &Filter) -> Vec<&'a Item> {
    let words: Vec<String> = f.query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut v: Vec<(u32, &Item)> = items
        .iter()
        .filter(|i| match f.price {
            PriceFilter::All => true,
            PriceFilter::Free => !i.entry.meta.pricing.model.needs_licence() && i.entry.meta.pricing.model != PriceModel::Paid,
            PriceFilter::Paid => i.entry.meta.pricing.model.needs_licence(),
        })
        .filter(|i| f.level.is_none_or(|l| i.verdict.level == l))
        .filter(|i| f.kind.as_ref().is_none_or(|k| i.entry.kind == *k))
        .filter(|i| f.category.as_ref().is_none_or(|c| i.entry.meta.categories.iter().any(|x| x == c)))
        .filter(|i| f.publisher.as_ref().is_none_or(|p| i.entry.meta.publisher.id == *p))
        .filter_map(|i| score(i, &words).map(|s| (s, i)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.entry.featured.cmp(&a.1.entry.featured)).then(a.1.title().to_lowercase().cmp(&b.1.title().to_lowercase())));
    v.into_iter().map(|(_, i)| i).collect()
}

pub fn featured(items: &[Item]) -> Vec<&Item> {
    items.iter().filter(|i| i.entry.featured && !i.state.is_installed()).collect()
}

/// Newest first, at most `n`.
pub fn newest(items: &[Item], n: usize) -> Vec<&Item> {
    let mut v: Vec<&Item> = items.iter().filter(|i| i.entry.added > 0 && !i.entry.demo).collect();
    v.sort_by(|a, b| b.entry.added.cmp(&a.entry.added).then(a.title().cmp(b.title())));
    v.truncate(n);
    v
}

pub fn categories(items: &[Item]) -> Vec<(String, usize)> {
    let mut m = std::collections::BTreeMap::<String, usize>::new();
    for i in items {
        for c in &i.entry.meta.categories {
            *m.entry(c.clone()).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublisherInfo {
    pub id: String,
    pub name: String,
    pub url: String,
    pub level: Level,
    pub apps: usize,
}

pub fn publishers(items: &[Item]) -> Vec<PublisherInfo> {
    let mut v: Vec<PublisherInfo> = Vec::new();
    for i in items {
        let p = &i.entry.meta.publisher;
        if p.id.is_empty() {
            continue;
        }
        match v.iter_mut().find(|x| x.id == p.id) {
            Some(x) => {
                x.apps += 1;
                x.level = x.level.min(i.verdict.level);
            }
            None => v.push(PublisherInfo { id: p.id.clone(), name: p.name.clone(), url: p.url.clone(), level: i.verdict.level, apps: 1 }),
        }
    }
    v.sort_by(|a, b| a.level.cmp(&b.level).then(a.name.cmp(&b.name)));
    v
}
