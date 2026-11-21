//! Reading a `&'static str` from the wire (F-SPLIT-03, the UI socket). Some answers of the core carry words that are `&'static str` in the
//! program (a kind of cluster object, a scope name). Over the socket they arrive as text, and each distinct word is interned once for the
//! life of the process, so a screen that asks every second does not grow the memory.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Deserializer};

static WORDS: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// The one `&'static str` of this text: the same pointer for the same words.
pub fn intern(s: &str) -> &'static str {
    let mut g = WORDS.lock().unwrap_or_else(PoisonError::into_inner);
    let set = g.get_or_insert_with(HashSet::new);
    if let Some(w) = set.get(s) {
        return w;
    }
    let w: &'static str = Box::leak(s.to_string().into_boxed_str());
    set.insert(w);
    w
}

/// `#[serde(deserialize_with = "crate::serde_static::keyed_words")]` on a `Vec<(T, Vec<&'static str>)>` field. (A field that is itself a
/// `&'static str` makes serde require `'de: 'static`, so a type with one reads a wire twin of its own and converts with [`intern`].)
pub fn keyed_words<'de, D, T>(d: D) -> Result<Vec<(T, Vec<&'static str>)>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Vec::<(T, Vec<String>)>::deserialize(d)?.into_iter().map(|(k, ws)| (k, ws.iter().map(|w| intern(w)).collect())).collect())
}

static LISTS: Mutex<Option<std::collections::HashMap<String, &'static [&'static str]>>> = Mutex::new(None);

/// The one `&'static [&'static str]` of these words: the same slice for the same words (a screen that asks every second does not grow the memory).
pub fn word_list(words: &[String]) -> &'static [&'static str] {
    let key = words.join("\u{1}");
    let mut g = LISTS.lock().unwrap_or_else(PoisonError::into_inner);
    let m = g.get_or_insert_with(Default::default);
    if let Some(l) = m.get(&key) {
        return l;
    }
    let list: Vec<&'static str> = words.iter().map(|w| intern(w)).collect();
    let l: &'static [&'static str] = Box::leak(list.into_boxed_slice());
    m.insert(key, l);
    l
}

/// `#[serde(deserialize_with = "crate::serde_static::words")]` on a `Vec<&'static str>` field.
pub fn words<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<&'static str>, D::Error> {
    Ok(Vec::<String>::deserialize(d)?.iter().map(|w| intern(w)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Row {
        #[serde(deserialize_with = "keyed_words")]
        rt: Vec<(u8, Vec<&'static str>)>,
    }

    #[test]
    fn static_words_come_back_from_text_and_the_same_word_is_the_same_pointer() {
        let r = Row { rt: vec![(1, vec!["node", "uv"])] };
        let j = serde_json::to_string(&r).unwrap();
        let a: Row = serde_json::from_str(&j).unwrap();
        let b: Row = serde_json::from_str(&j).unwrap();
        assert_eq!(a, r);
        assert!(std::ptr::eq(a.rt[0].1[1], b.rt[0].1[1]));
        assert!(std::ptr::eq(intern("Deployment"), intern(&String::from("Deployment"))));
    }

    #[test]
    fn a_list_of_static_words_comes_back_from_text() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct Archs {
            #[serde(deserialize_with = "words")]
            archs: Vec<&'static str>,
        }
        let a = Archs { archs: vec!["amd64", "arm64"] };
        let back: Archs = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        assert!(std::ptr::eq(back.archs[0], intern("amd64")));
        let (a, b) = (word_list(&["x".to_string(), "y".to_string()]), word_list(&["x".to_string(), "y".to_string()]));
        assert!(std::ptr::eq(a, b) && a == ["x", "y"], "the same words are the same slice");
    }
}
