//! Port-forwards on the K8S tile (F-K8S-26): the plain rows and request the tile shows and sends (mirrors of the core's `portforward` types, so the
//! tile names no core crate), the words a person types (`svc/web 80`, `pod/web-1 18080:8080`, `deploy/api 3000 18080`), and the window with the
//! running forwards, a field to start one and a key to stop one. Every start goes through [`crate::port::K8sPort::start_forward`], which is the
//! core's facet: the person's card for the capability `k8s.portforward` is asked there, so the tile never starts a process itself.

use crate::ui::{Key, TextField};

/// One forward that runs (a mirror of `iyakki_core_api::portforward::Info`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwdRow {
    pub id: u32,
    pub namespace: String,
    /// `svc/web`, `pod/web-1`, `deployment/api`.
    pub target: String,
    pub remote: u16,
    pub local: u16,
}

impl FwdRow {
    pub fn line(&self) -> String {
        format!("#{} 127.0.0.1:{} -> {}/{}:{}", self.id, self.local, self.namespace, self.target, self.remote)
    }
}

/// A forward to start (a mirror of `iyakki_core_api::portforward::Request`): `kind` is `pod`, `svc` or `deployment`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwdSpec {
    pub kubeconfig: String,
    pub context: String,
    pub namespace: String,
    pub kind: String,
    pub name: String,
    pub remote: u16,
    pub local: Option<u16>,
}

fn kind_of(word: &str) -> Option<&'static str> {
    match word.to_ascii_lowercase().as_str() {
        "pod" | "pods" | "po" => Some("pod"),
        "service" | "services" | "svc" => Some("svc"),
        "deployment" | "deployments" | "deploy" => Some("deployment"),
        _ => None,
    }
}

fn port_of(s: &str) -> Result<u16, String> {
    s.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| format!("`{s}` is not a port (1 to 65535)"))
}

/// What the person typed: `kind/name remote`, `kind/name local:remote` or `kind/name remote local`. Returns (kind, name, remote, local).
pub fn parse_spec(text: &str) -> Result<(String, String, u16, Option<u16>), String> {
    let mut words = text.split_whitespace();
    let obj = words.next().ok_or("type what to forward to, such as `svc/web 80`")?;
    let (k, name) = obj.split_once('/').ok_or("name the object as kind/name, such as `svc/web` or `pod/web-1`")?;
    let kind = kind_of(k).ok_or("the kind is pod, svc or deployment")?;
    if name.is_empty() {
        return Err("the object has no name after the slash".into());
    }
    let first = words.next().ok_or("add the port on the object, such as `svc/web 80`")?;
    let (remote, mut local) = match first.split_once(':') {
        Some((l, r)) => (port_of(r)?, Some(port_of(l)?)),
        None => (port_of(first)?, None),
    };
    if let Some(third) = words.next() {
        if local.is_some() {
            return Err("give the local port once: `18080:80` or `80 18080`".into());
        }
        local = Some(port_of(third)?);
    }
    if words.next().is_some() {
        return Err("that is more than an object, a port and a local port".into());
    }
    Ok((kind.to_string(), name.to_string(), remote, local))
}

/// The item the Terminal is handed to open a shell in a pod (`pod:` and six lines: kubeconfig, context, namespace, pod, container, shell). The Terminal
/// view reads it with `PodSession::from_item`; the core checks every part again before anything runs.
pub fn pod_item(kubeconfig: &str, context: &str, namespace: &str, pod: &str, container: &str) -> String {
    format!("pod:{kubeconfig}\n{context}\n{namespace}\n{pod}\n{container}\n")
}

/// The window: the forwards that run, the field and a word of the last result.
pub struct FwdWindow {
    pub field: TextField,
    pub rows: Vec<FwdRow>,
    pub sel: usize,
    pub note: String,
}

impl FwdWindow {
    pub fn new(rows: Vec<FwdRow>, prefill: &str) -> FwdWindow {
        let mut field = TextField::new("svc/web 80", false);
        if !prefill.is_empty() {
            field.set(prefill);
        }
        FwdWindow { field, rows, sel: 0, note: String::new() }
    }

    pub fn set_rows(&mut self, rows: Vec<FwdRow>) {
        self.rows = rows;
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&FwdRow> {
        self.rows.get(self.sel)
    }

    /// Up and Down move the selection; every other key goes to the field. (Enter, Delete and Esc are the tile's.)
    pub fn key(&mut self, k: &Key) {
        match k {
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(self.rows.len().saturating_sub(1)),
            other => {
                self.field.handle(other);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_a_person_types_give_an_object_a_port_and_maybe_a_local_port() {
        assert_eq!(parse_spec("svc/web 80").unwrap(), ("svc".into(), "web".into(), 80, None));
        assert_eq!(parse_spec("Service/web 18080:80").unwrap(), ("svc".into(), "web".into(), 80, Some(18080)));
        assert_eq!(parse_spec("deploy/api 3000 18080").unwrap(), ("deployment".into(), "api".into(), 3000, Some(18080)));
        assert_eq!(parse_spec("  po/web-1   8080 ").unwrap().0, "pod");
    }

    #[test]
    fn what_cannot_be_a_forward_is_said_in_words() {
        for (bad, word) in [("", "type what"), ("web 80", "kind/name"), ("node/n1 80", "pod, svc or deployment"), ("svc/ 80", "no name"), ("svc/web", "add the port"), ("svc/web 0", "not a port"), ("svc/web x", "not a port"), ("svc/web 99999", "not a port"), ("svc/web 1:2 3", "once"), ("svc/web 80 81 82", "more than")] {
            let e = parse_spec(bad).unwrap_err();
            assert!(e.contains(word), "{bad}: {e}");
        }
    }

    #[test]
    fn the_item_for_the_terminal_has_six_parts_and_an_empty_shell() {
        assert_eq!(pod_item("/k/c.yaml", "dev", "shop", "web-1", "app"), "pod:/k/c.yaml\ndev\nshop\nweb-1\napp\n");
        assert_eq!(pod_item("", "", "a", "b", "").split('\n').count(), 6);
    }

    #[test]
    fn the_window_keeps_its_selection_inside_the_rows_and_gives_other_keys_to_the_field() {
        let row = |id| FwdRow { id, namespace: "shop".into(), target: "svc/web".into(), remote: 80, local: 40000 + id as u16 };
        let mut w = FwdWindow::new(vec![row(1), row(2)], "svc/web ");
        assert_eq!(w.field.value, "svc/web ");
        w.key(&Key::Down);
        w.key(&Key::Down);
        assert_eq!(w.selected().unwrap().id, 2);
        w.key(&Key::Up);
        w.key(&Key::Up);
        assert_eq!(w.selected().unwrap().id, 1);
        w.key(&Key::Char('8'));
        assert_eq!(w.field.value, "svc/web 8");
        w.sel = 1;
        w.set_rows(vec![row(3)]);
        assert_eq!((w.sel, w.selected().unwrap().line()), (0, "#3 127.0.0.1:40003 -> shop/svc/web:80".to_string()));
        w.set_rows(vec![]);
        assert!(w.selected().is_none());
    }
}
