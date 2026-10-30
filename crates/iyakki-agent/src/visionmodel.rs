//! Which model looks at a picture. Every step that shows a picture to a model (a flow's agent step with `images`, the camera
//! step's pre-flight check) asks [`Services::vision_model`] instead of assuming the chat model can see:
//!
//! 1. the step's own model, when it can see pictures (flows have none today);
//! 2. the person's choice, **Settings -> Models -> Vision model** (`model.vision`);
//! 3. the chat model, when it can see pictures;
//! 4. otherwise one is picked from the providers' own lists (no network call: the lists saved at the last refresh): the chat model's
//!    provider first, then the others, on this computer before on the network; models the server labels vision before models only
//!    the name says can see; Qwen vision names before the rest; the smallest download first when sizes are known. The pick is kept for
//!    the session per chat provider until the lists are refreshed.
//!
//! **A cloud model is never picked silently** for a camera picture: when the only vision model found is on a cloud service, the
//! step stops with a "needs your choice" message and the person chooses it in Settings -> Models (which is the confirmation).
//! When none is found the message says so plainly, with the model names to install.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::providers::{model_sees_pictures, Kind, ProviderCfg};
use crate::services::Services;
use crate::settings::ModelChoice;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    /// On this computer.
    Local,
    /// On another machine of the network (a private address or a `.local` name).
    Network,
    /// A service on the internet.
    Cloud,
}

impl Place {
    /// Where the model is, with the host of its provider: the words used wherever the person is told where a picture goes.
    pub fn describe(self, host: &str) -> String {
        match self {
            Place::Local => "on this computer".into(),
            Place::Network => format!("on another computer on your network ({host})"),
            Place::Cloud => format!("a CLOUD service ({host})"),
        }
    }
}

/// Where a provider is and its host, from its kind and address. Loopback and unix sockets are this computer; a private or link-local
/// address, a `.local` name or a cluster address of another cluster is another computer on the network; anything else is a cloud service.
pub fn place_and_host(kind: Kind, base_url: Option<&str>) -> (Place, String) {
    let named = base_url.map(str::trim).filter(|u| !u.is_empty()).or_else(|| kind.default_base());
    if matches!(kind, Kind::OpenAi | Kind::OpenRouter | Kind::Anthropic | Kind::Gemini) {
        let host = named.and_then(|u| iyakki_sec::url_host(u)).unwrap_or_else(|| kind.label().to_string());
        return (Place::Cloud, host);
    }
    let Some(url) = named else { return (Place::Cloud, "an unknown address".into()) };
    if url.starts_with("unix:") || url.starts_with("unix+") {
        return (Place::Local, "this computer".into());
    }
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map_or(auth, |x| x.1);
    let host = auth.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(auth, |x| x.0).trim_matches(['[', ']']).to_ascii_lowercase();
    if scheme.eq_ignore_ascii_case("cluster") {
        // the local cluster is this computer; any other cluster is another computer
        return if matches!(host.as_str(), "" | "local" | "localhost") { (Place::Local, host) } else { (Place::Network, host) };
    }
    let v4 = host.parse::<std::net::Ipv4Addr>().ok();
    let v6 = host.parse::<std::net::Ipv6Addr>().ok();
    if host == "localhost" || v4.is_some_and(|a| a.is_loopback()) || v6.is_some_and(|a| a.is_loopback()) {
        (Place::Local, host)
    } else if host.ends_with(".local") || v4.is_some_and(|a| a.is_private() || a.is_link_local()) || v6.is_some_and(|a| (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80) {
        (Place::Network, host)
    } else {
        (Place::Cloud, host)
    }
}

/// Where a provider is (see [`place_and_host`]).
pub fn place_of(kind: Kind, base_url: Option<&str>) -> Place {
    place_and_host(kind, base_url).0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Own,
    Setting,
    Chat,
    Auto,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub provider: String,
    pub model: String,
    pub place: Place,
    /// The host of the provider's address (what the person sees next to "on another computer on your network").
    pub host: String,
    pub how: How,
}

impl Pick {
    pub fn choice(&self) -> ModelChoice {
        ModelChoice { provider: self.provider.clone(), model: self.model.clone() }
    }
    /// "Using qwen2.5-vl-7b on lemonade-local: on this computer" (shown in the log and the permission text).
    pub fn line(&self) -> String {
        format!("Using {} on {}: {}{}.", self.model, self.provider, self.place.describe(&self.host), match self.how {
            How::Setting => " (your Vision model setting)",
            How::Chat => " (the chat model)",
            How::Auto => " (picked automatically)",
            How::Own => "",
        })
    }
}

/// A model that can see pictures, as a provider's list says.
#[derive(Clone, Debug, PartialEq)]
pub struct Cand {
    pub provider: String,
    pub model: String,
    pub place: Place,
    pub host: String,
    /// The server itself labels it vision (not only its name).
    pub labelled: bool,
    pub size_gb: Option<f64>,
    /// In the server's memory now (otherwise the first call has to load it, which takes time and memory).
    pub loaded: bool,
}

impl Cand {
    /// What the server says comes first (its `vision` label is trusted even when the name shows nothing, `Qwen3.5-4B-MTP-GGUF`,
    /// `Gemma-4-E2B-it-GGUF`): 0 labelled Qwen, 1 other labelled, 2 name-only Qwen vision, 3 other names.
    fn group(&self) -> u8 {
        let m = self.model.to_ascii_lowercase();
        match (self.labelled, m.contains("qwen")) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        }
    }
    pub fn label(&self) -> String {
        format!("{}/{}  ({})", self.provider, self.model, self.place.describe(&self.host))
    }
}

/// Why no vision model could be chosen; both are "needs your choice" failures (the fixing agent is not asked).
#[derive(Clone, Debug, PartialEq)]
pub enum VisionError {
    NoneFound(String),
    CloudOnly(String),
}

impl VisionError {
    pub fn message(&self) -> &str {
        match self {
            VisionError::NoneFound(m) | VisionError::CloudOnly(m) => m,
        }
    }
}

fn cache() -> &'static Mutex<HashMap<String, (String, String)>> {
    static C: OnceLock<Mutex<HashMap<String, (String, String)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn install_hint(kind: Option<Kind>) -> &'static str {
    match kind {
        Some(Kind::Ollama) => "for example `ollama pull qwen2.5vl:7b`",
        Some(Kind::Lemonade) => "for example a Qwen vision model (Qwen2.5-VL or Qwen3-VL; Settings -> Models, key l, lists what the server offers)",
        _ => "for example a Qwen vision model (Qwen2.5-VL or Qwen3-VL)",
    }
}

impl Services {
    fn provider_cands(&self, p: &ProviderCfg) -> Vec<Cand> {
        let (place, host) = place_and_host(p.kind, p.base_url.as_deref());
        let mk = |model: String, labelled: bool, size_gb: Option<f64>, loaded: bool| Cand { provider: p.id.clone(), model, place, host: host.clone(), labelled, size_gb, loaded };
        if p.kind == Kind::Lemonade {
            self.lemonade_cached(&p.id)
                .into_iter()
                .filter(|m| m.downloaded && m.kind.chats())
                .filter(|m| m.kind == crate::lemonade::ModelType::Vision || m.labels.iter().any(|l| l.eq_ignore_ascii_case("vision")) || model_sees_pictures(&m.id))
                .map(|m| {
                    let labelled = m.kind == crate::lemonade::ModelType::Vision || m.labels.iter().any(|l| l.eq_ignore_ascii_case("vision"));
                    mk(m.id, labelled, m.size_gb, m.loaded)
                })
                .collect()
        } else {
            p.models.iter().filter(|m| model_sees_pictures(m)).map(|m| mk(m.clone(), false, None, false)).collect()
        }
    }

    /// Every vision-capable model the enabled providers listed at the last refresh, in the order the automatic pick prefers them.
    pub fn vision_candidates(&self) -> Vec<Cand> {
        let (provs, chat) = {
            let g = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            (g.get().providers.clone(), g.get().chat.clone())
        };
        let mut v: Vec<Cand> = provs.iter().filter(|p| p.enabled).flat_map(|p| self.provider_cands(p)).collect();
        let chat_provider = chat.map(|c| c.provider).unwrap_or_default();
        v.sort_by(|a, b| {
            let key = |c: &Cand| (u8::from(c.provider != chat_provider), c.place, c.group());
            key(a).cmp(&key(b)).then_with(|| a.size_gb.unwrap_or(f64::MAX).partial_cmp(&b.size_gb.unwrap_or(f64::MAX)).unwrap_or(std::cmp::Ordering::Equal)).then_with(|| a.model.cmp(&b.model))
        });
        v
    }

    /// The model that looks at a picture (see the module text for the order). `own` is the step's own model, if it has one.
    pub fn vision_model(&self, own: Option<&ModelChoice>) -> Result<Pick, VisionError> {
        let (chat, setting, provs) = {
            let g = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            (g.get().chat.clone(), g.get().vision.clone(), g.get().providers.clone())
        };
        let place = |id: &str| provs.iter().find(|p| p.id == id).map_or((Place::Cloud, "an unknown provider".to_string()), |p| place_and_host(p.kind, p.base_url.as_deref()));
        let pick = |c: &ModelChoice, how: How| {
            let (pl, host) = place(&c.provider);
            Pick { provider: c.provider.clone(), model: c.model.clone(), place: pl, host, how }
        };
        if let Some(o) = own.filter(|o| model_sees_pictures(&o.model)) {
            return Ok(pick(o, How::Own));
        }
        if let Some(s) = setting.as_ref().filter(|s| provs.iter().any(|p| p.id == s.provider && p.enabled)) {
            return Ok(pick(s, How::Setting));
        }
        if let Some(c) = chat.as_ref().filter(|c| model_sees_pictures(&c.model)) {
            return Ok(pick(c, How::Chat));
        }
        let chat_provider = chat.as_ref().map(|c| c.provider.clone()).unwrap_or_default();
        let key = format!("{}|{chat_provider}", self.dir().display());
        if let Some((p, m)) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned() {
            if provs.iter().any(|x| x.id == p && x.enabled) {
                let (pl, host) = place(&p);
                return Ok(Pick { place: pl, host, provider: p, model: m, how: How::Auto });
            }
        }
        let cands = self.vision_candidates();
        if let Some(c) = cands.iter().find(|c| c.place != Place::Cloud) {
            cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key, (c.provider.clone(), c.model.clone()));
            return Ok(Pick { provider: c.provider.clone(), model: c.model.clone(), place: c.place, host: c.host.clone(), how: How::Auto });
        }
        if let Some(c) = cands.first() {
            return Err(VisionError::CloudOnly(format!("The only vision model found is {} on {} ({}), a CLOUD service, and a camera picture is not sent to a cloud service without your say-so. Choose it as the Vision model in Settings -> Models if you want that, or install a vision model on this computer or network, {}, then press Refresh models.", c.model, c.provider, c.host, install_hint(provs.iter().find(|p| p.id == c.provider).map(|p| p.kind)))));
        }
        let at = if chat_provider.is_empty() { provs.iter().find(|p| p.enabled).map(|p| p.id.clone()).unwrap_or_else(|| "any provider".into()) } else { chat_provider.clone() };
        let kind = provs.iter().find(|p| p.id == at).map(|p| p.kind);
        Err(VisionError::NoneFound(format!("No vision model found on {at}. Install one, {}, then press Refresh models. Or choose one in Settings -> Models (Vision model).", install_hint(kind))))
    }

    /// Forget the automatic pick (the lists were refreshed).
    pub fn clear_vision_cache(&self) {
        let prefix = format!("{}|", self.dir().display());
        cache().lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| !k.starts_with(&prefix));
    }

    /// Read the model lists of the enabled providers again (Lemonade's typed list, the others' plain list) and forget the pick.
    /// Returns a line for the person.
    pub fn refresh_model_lists(&self) -> String {
        let provs: Vec<ProviderCfg> = self.settings.lock().unwrap_or_else(|e| e.into_inner()).get().providers.iter().filter(|p| p.enabled).cloned().collect();
        let (mut ok, mut failed) = (0, Vec::new());
        for p in provs {
            let r = if p.kind == Kind::Lemonade {
                self.refresh_lemonade(&p.id).map(|_| ())
            } else {
                self.client_for(&p.id, true).and_then(|c| c.list_models()).and_then(|list| {
                    let mut st = self.settings.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(mut cfg) = st.get().provider(&p.id).cloned() {
                        cfg.models = list;
                        st.upsert_provider(cfg).map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })
            };
            match r {
                Ok(()) => ok += 1,
                Err(e) => failed.push(format!("{}: {}", p.id, e.chars().take(80).collect::<String>())),
            }
        }
        self.clear_vision_cache();
        if failed.is_empty() { format!("Refreshed {ok} provider(s).") } else { format!("Refreshed {ok}; could not read {}.", failed.join("; ")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lemonade::{Model, ModelType};
    use std::sync::Arc;

    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn svc(tag: &str) -> Arc<Services> {
        let d = std::env::temp_dir().join(format!("iyakki-vision-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap()
    }

    fn lem(id: &str, kind: ModelType, gb: f64, loaded: bool) -> Model {
        Model { id: id.into(), kind, recipe: String::new(), checkpoint: String::new(), labels: if kind == ModelType::Vision { vec!["vision".into()] } else { vec![] }, downloaded: true, loaded, suggested: false, size_gb: Some(gb), image_defaults: None }
    }

    fn provider(s: &Services, id: &str, kind: Kind, base: &str, models: &[Model]) {
        let mut p = ProviderCfg::new(id, kind);
        p.base_url = Some(base.into());
        if kind.needs_key() {
            p.key_ref = Some(format!("vault:{id}-key"));
        }
        if kind == Kind::Lemonade {
            crate::lemonade::remember(&crate::lemonade::api_base(base), models);
        } else {
            p.models = models.iter().map(|m| m.id.clone()).collect();
        }
        s.settings.lock().unwrap().upsert_provider(p).unwrap();
    }

    fn chat(s: &Services, p: &str, m: &str) {
        s.settings.lock().unwrap().update(|x| x.chat = Some(ModelChoice { provider: p.into(), model: m.into() })).unwrap();
    }

    /// The Mac's Lemonade at another address: two vision-labelled models whose names do not say vision, a chat model that cannot see.
    fn mac_like(tag: &str, port: u16) -> Arc<Services> {
        let s = svc(tag);
        let base = format!("http://192.168.50.68:{port}");
        provider(&s, "lemonde", Kind::Lemonade, &base, &[lem("qwen3-coder-30b", ModelType::Chat, 17.0, true), lem("Gemma-4-E2B-it-GGUF", ModelType::Vision, 3.0, false), lem("Qwen3.5-4B-MTP-GGUF", ModelType::Vision, 3.5, false), lem("qwen3-8b", ModelType::Chat, 5.0, false), lem("SD-Turbo", ModelType::Image, 2.0, true)]);
        chat(&s, "lemonde", "qwen3-coder-30b");
        s
    }

    #[test]
    fn where_a_provider_is_comes_from_its_address_in_three_classes_and_never_says_this_computer_for_another_one() {
        let cases = [
            ("http://127.0.0.1:13305", Kind::Lemonade, Place::Local, "on this computer"),
            ("http://localhost:11434", Kind::Ollama, Place::Local, "on this computer"),
            ("http://192.168.50.68:31435", Kind::Lemonade, Place::Network, "on another computer on your network (192.168.50.68)"),
            ("http://10.1.2.3:8000/v1", Kind::Compat, Place::Network, "on another computer on your network (10.1.2.3)"),
            ("http://nas.local:8000", Kind::Compat, Place::Network, "on another computer on your network (nas.local)"),
            ("cluster://devbox/lemonade", Kind::Compat, Place::Network, "on another computer on your network (devbox)"),
            ("cluster://local/lemonade", Kind::Compat, Place::Local, "on this computer"),
            ("https://lemonade.example.test/api/v1", Kind::Lemonade, Place::Cloud, "a CLOUD service (lemonade.example.test)"),
        ];
        for (url, kind, place, words) in cases {
            let (p, host) = place_and_host(kind, Some(url));
            assert_eq!(p, place, "{url}");
            assert_eq!(p.describe(&host), words, "{url}");
            assert_eq!(place.describe(&host).contains("this computer"), place == Place::Local, "{url}");
        }
        assert_eq!(place_and_host(Kind::OpenAi, None).0, Place::Cloud);
        let pick = Pick { provider: "p".into(), model: "m".into(), place: Place::Network, host: "192.168.50.68".into(), how: How::Auto };
        assert_eq!(pick.line(), "Using m on p: on another computer on your network (192.168.50.68) (picked automatically).");
    }

    #[test]
    fn the_auto_pick_trusts_the_servers_vision_label_and_prefers_qwen_then_the_rest_and_says_where() {
        let s = mac_like("mac", 31435);
        let p = s.vision_model(None).unwrap();
        assert_eq!((p.model.as_str(), p.provider.as_str(), p.how), ("Qwen3.5-4B-MTP-GGUF", "lemonde", How::Auto));
        assert_eq!((p.place, p.host.as_str()), (Place::Network, "192.168.50.68"));
        let names: Vec<String> = s.vision_candidates().into_iter().map(|c| c.model).collect();
        assert_eq!(names, vec!["Qwen3.5-4B-MTP-GGUF", "Gemma-4-E2B-it-GGUF"], "labelled Qwen first, then the other labelled; chat and image models are not listed");
        // a model the server has to load first is known as such
        assert!(s.vision_candidates().iter().all(|c| !c.loaded));
    }

    #[test]
    fn the_chat_models_provider_comes_first_then_local_before_network_and_cloud_is_never_picked_by_itself() {
        let s = mac_like("order", 31436);
        provider(&s, "lemonade-local", Kind::Lemonade, "http://127.0.0.1:13305", &[lem("Qwen2.5-VL-7B-Instruct-GGUF", ModelType::Vision, 6.0, false)]);
        assert_eq!(s.vision_model(None).unwrap().provider, "lemonde", "the chat model's provider first, even though the other is on this computer");
        // the chat provider has none: the one on this computer
        let s = svc("order2");
        provider(&s, "lemonde", Kind::Lemonade, "http://192.168.50.68:31437", &[lem("qwen3-coder-30b", ModelType::Chat, 17.0, true)]);
        provider(&s, "lemonade-local", Kind::Lemonade, "http://127.0.0.1:13306", &[lem("Qwen2.5-VL-3B-Instruct-GGUF", ModelType::Vision, 3.3, false)]);
        provider(&s, "far", Kind::Lemonade, "https://lemonade.example.test", &[lem("Qwen3-VL-8B-Instruct-GGUF", ModelType::Vision, 6.2, false)]);
        chat(&s, "lemonde", "qwen3-coder-30b");
        let p = s.vision_model(None).unwrap();
        assert_eq!((p.provider.as_str(), p.place), ("lemonade-local", Place::Local));
        assert!(p.line().contains("on this computer") && !p.line().contains("CLOUD"));
        // only a cloud one is left: the step must ask, naming it and its host
        s.settings.lock().unwrap().update(|x| x.providers.retain(|q| q.id != "lemonade-local")).unwrap();
        s.clear_vision_cache();
        let e = s.vision_model(None).unwrap_err();
        assert!(matches!(e, VisionError::CloudOnly(_)) && e.message().contains("Qwen3-VL-8B-Instruct-GGUF") && e.message().contains("lemonade.example.test") && e.message().contains("CLOUD"), "{}", e.message());
        // the person's own choice of a cloud model is their confirmation
        s.settings.lock().unwrap().update(|x| x.vision = Some(ModelChoice { provider: "far".into(), model: "Qwen3-VL-8B-Instruct-GGUF".into() })).unwrap();
        let p = s.vision_model(None).unwrap();
        assert_eq!((p.how, p.place), (How::Setting, Place::Cloud));
    }

    #[test]
    fn the_setting_beats_the_chat_model_and_a_seeing_chat_model_beats_the_auto_pick() {
        let s = svc("setting");
        provider(&s, "p", Kind::Ollama, "http://localhost:11434", &[lem("qwen2.5vl:7b", ModelType::Chat, 0.0, false), lem("llava:13b", ModelType::Chat, 0.0, false)]);
        chat(&s, "p", "qwen2.5vl:7b");
        assert_eq!(s.vision_model(None).unwrap().how, How::Chat);
        s.settings.lock().unwrap().update(|x| x.vision = Some(ModelChoice { provider: "p".into(), model: "llava:13b".into() })).unwrap();
        let p = s.vision_model(None).unwrap();
        assert_eq!((p.model.as_str(), p.how), ("llava:13b", How::Setting));
        // the step's own model that can see is first of all
        let own = ModelChoice { provider: "p".into(), model: "qwen2.5vl:7b".into() };
        assert_eq!(s.vision_model(Some(&own)).unwrap().how, How::Own);
    }

    #[test]
    fn nothing_found_says_so_plainly_with_what_to_install_and_the_pick_is_cached_until_a_refresh() {
        let s = svc("none");
        provider(&s, "p", Kind::Ollama, "http://localhost:11434", &[lem("llama3:8b", ModelType::Chat, 0.0, false)]);
        chat(&s, "p", "llama3:8b");
        let e = s.vision_model(None).unwrap_err();
        assert!(matches!(e, VisionError::NoneFound(_)) && e.message().starts_with("No vision model found on p.") && e.message().contains("ollama pull qwen2.5vl") && e.message().contains("Refresh models"), "{}", e.message());
        // a vision model appears in the list: still the cached "none" is not kept (only picks are), and the pick is kept afterwards
        provider(&s, "p", Kind::Ollama, "http://localhost:11434", &[lem("llama3:8b", ModelType::Chat, 0.0, false), lem("qwen2.5vl:7b", ModelType::Chat, 0.0, false)]);
        let p = s.vision_model(None).unwrap();
        assert_eq!(p.model, "qwen2.5vl:7b");
        // the list changes under it: the old pick stays for the session ...
        provider(&s, "p", Kind::Ollama, "http://localhost:11434", &[lem("llama3:8b", ModelType::Chat, 0.0, false), lem("minicpm-v:8b", ModelType::Chat, 0.0, false)]);
        assert_eq!(s.vision_model(None).unwrap().model, "qwen2.5vl:7b", "cached");
        // ... until the lists are refreshed
        s.clear_vision_cache();
        assert_eq!(s.vision_model(None).unwrap().model, "minicpm-v:8b");
    }

    #[test]
    fn the_vision_model_setting_is_in_the_developer_config_whitelist() {
        let s = svc("cfg");
        provider(&s, "p", Kind::Ollama, "http://localhost:11434", &[]);
        let def = crate::devmode::config::def("model.vision").expect("whitelisted");
        assert!(def.desc.contains("PROVIDER/MODEL"));
        let mut st = s.settings.lock().unwrap().get().clone();
        crate::devmode::config::set_for_test(&mut st, "model.vision", "p/qwen2.5vl:7b").unwrap();
        assert_eq!(st.vision.as_ref().map(|c| c.model.as_str()), Some("qwen2.5vl:7b"));
        assert!(crate::devmode::config::set_for_test(&mut st, "model.vision", "nope/x").is_err());
    }
}
