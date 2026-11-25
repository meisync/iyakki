//! Settings -> Models: the provider's own model list (key L for chat, E for embeddings) and the request options of a model
//! (key o): `max_tokens`, `temperature`, `top_p`, `reasoning_budget`. The list comes from the provider's `GET {base}/models` with the
//! key from the vault (never shown); the person picks a model instead of typing it, and the pick is saved exactly as the Models
//! picker (m) saves it. The options are one row each in a chooser, edited in a small form; the rules are in `iyakki_agent::modelopts`.

use iyakki_shared::modelopts::{FIELDS};
use iyakki_shared::modelopts::{self, ModelOptions};

use super::*;
use super::apps_port::ModelOptsPort;

/// The list a provider answered with (or why it could not), for the job result.
pub(super) struct MoList {
    pub(super) provider: String,
    pub(super) embed: bool,
    pub(super) result: Result<Vec<String>, String>,
}

/// The form of one option: provider, model, option name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MoForm {
    Field(String, String, String),
}

const CLEAR_ROW: &str = "Clear all options for this model";

impl SettingsApp {
    /// The model whose options the page shows for this provider: the chat model when it is this provider's, else its saved default.
    pub(super) fn mo_model(p: &dyn ModelOptsPort, prov: &ProviderCfg) -> Option<String> {
        let st = p.settings_get();
        st.chat.as_ref().filter(|c| c.provider == prov.id).map(|c| c.model.clone()).or_else(|| prov.default_model.clone())
    }

    /// Ask the provider for its model list (a job: it is a network call). Admin only: it uses the provider's key.
    pub(super) fn mo_list(&mut self, p: &dyn ModelOptsPort, prov: &ProviderCfg, embed: bool) {
        let fetcher = p.model_fetcher();
        let id = prov.id.clone();
        self.start(&format!("asking {id} for its models"), move || {
            let result = fetcher.fetch(&id);
            JobOut::ModelList(MoList { provider: id, embed, result })
        });
    }

    /// The list arrived: open the chooser (with its search box), or say why not.
    pub(super) fn mo_list_done(&mut self, p: &dyn ModelOptsPort, out: MoList) {
        let _ = p;
        let MoList { provider, embed, result } = out;
        match result {
            Err(e) => self.fail(format!("{provider}: {e}")),
            Ok(ids) if ids.is_empty() => self.fail(format!("{provider} listed no models for this key.")),
            Ok(ids) => {
                let mut items = ids;
                if embed {
                    items.insert(0, "(built-in hashing embeddings)".into());
                }
                let title = format!("{} model on {provider} ({} offered; type to search)", if embed { "Embedding" } else { "Chat" }, items.len() - usize::from(embed));
                let kind = if embed { PickKind::Embed(provider) } else { PickKind::Chat(provider) };
                self.modal = Some(Modal::Pick(kind, Picker::new(&title, items)));
            }
        }
    }

    /// The rows of the options chooser.
    pub(super) fn mo_rows(p: &dyn ModelOptsPort, prov: &ProviderCfg, model: &str) -> Vec<String> {
        let o = modelopts::get(&p.data_dir(), &prov.id, model);
        let mut rows: Vec<String> = FIELDS
            .iter()
            .map(|f| {
                let note = modelopts::used_by(prov.kind, f).err().map(|w| format!("  ({w})")).unwrap_or_default();
                format!("{f:<17} {}{note}", o.show(f))
            })
            .collect();
        rows.push(CLEAR_ROW.into());
        rows
    }

    /// Key o: the options of this provider's model, one row each.
    pub(super) fn mo_options(&mut self, p: &dyn ModelOptsPort, prov: &ProviderCfg) {
        let Some(model) = Self::mo_model(p, prov) else {
            return self.fail("Choose a chat model for this provider first (key m, or L to list its models).");
        };
        let rows = Self::mo_rows(p, prov, &model);
        self.modal = Some(Modal::Pick(PickKind::Options(prov.id.clone()), Picker::new(&format!("Request options for {} on {}", model, prov.id), rows)));
    }

    /// A row was chosen: the clearing row clears, an option row opens its form (administrators only).
    pub(super) fn mo_pick(&mut self, p: &dyn ModelOptsPort, provider: &str, item: &str) {
        let Some(prov) = p.settings_get().providers.into_iter().find(|prov| prov.id == provider) else { return };
        let Some(model) = Self::mo_model(p, &prov) else { return };
        if item == CLEAR_ROW {
            return match modelopts::set(&p.data_dir(), provider, &model, &ModelOptions::default()) {
                Ok(()) => {
                    p.audit("settings.model_options", &format!("{provider}/{model} cleared"));
                    self.ok(format!("{provider} · {model}: request options cleared; the provider's own defaults apply."));
                }
                Err(e) => self.fail(e),
            };
        }
        let Some(field) = FIELDS.iter().find(|f| item.starts_with(**f)) else { return };
        let o = modelopts::get(&p.data_dir(), provider, &model);
        let cur = if o.has(field) { o.show(field).split(' ').next().unwrap_or("").to_string() } else { String::new() };
        let help = match *field {
            "max_tokens" => format!("Most tokens in one reply, 1 to {}. Empty: the provider's default.", modelopts::MAX_TOKENS_MAX),
            "temperature" => "0 to 2. Lower is steadier, higher is more varied. Empty: the provider's default.".to_string(),
            "top_p" => "0 to 1. Empty: the provider's default.".to_string(),
            _ => match modelopts::used_by(prov.kind, field) {
                Ok(()) => "Tokens the model may think with; 0 turns thinking off. Empty: the provider's default.".to_string(),
                Err(w) => format!("{w}: it is saved but not sent to {}.", prov.kind.label()),
            },
        };
        let f = Form::new(&format!("{field} for {model}")).with_note(&help).text(field, &cur, "a number; empty clears it");
        self.open_form(FormKind::ModelOpt(MoForm::Field(provider.into(), model, (*field).into())), f);
    }

    /// The form of one option was submitted.
    pub(super) fn mo_submit(&mut self, p: &dyn ModelOptsPort, f: &MoForm, form: &Form) -> Result<String, String> {
        if !self.admin(p) {
            return Err("Only an administrator changes model options.".into());
        }
        let MoForm::Field(provider, model, field) = f;
        let dir = p.data_dir();
        let mut o = modelopts::get(&dir, provider, model);
        o.set_field(field, &form.value(0))?;
        modelopts::set(&dir, provider, model, &o)?;
        p.audit("settings.model_options", &format!("{provider}/{model} {field}"));
        Ok(format!("{provider} · {model}: {field} {}.", o.show(field)))
    }

    /// Under the provider list: the options in one line, the "not offered" warning, and the buttons (list, options). The area ends
    /// at `a.y + a.h` when it fits; the page scrolls when it does not.
    pub(super) fn mo_render(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn ModelOptsPort, prov: &ProviderCfg, admin: bool) {
        let mut y = a.y;
        let model = Self::mo_model(p, prov);
        let lh = 20.0 * s;
        if let Some(m) = &model {
            let o = modelopts::get(&p.data_dir(), &prov.id, m);
            let line = format!("Options for {m}: {}  [o]", o.summary());
            let fitted = g.fit(13.0 * s, &line, Face::Sans, (a.w - 8.0 * s).max(40.0));
            g.text(a.x + 4.0 * s, y, 13.0 * s, MUTED, &fitted, Face::Sans);
            g.hit(R::new(a.x, y - 2.0 * s, a.w, lh), Event::Key(Key::Char('o')), &format!("Request options: {line}"));
            y += lh;
            if let Some(w) = modelopts::not_offered(&prov.id, m) {
                let fitted = g.fit(13.0 * s, &w, Face::Bold, (a.w - 8.0 * s).max(40.0));
                g.text(a.x + 4.0 * s, y, 13.0 * s, crate::fx::AMBER, &fitted, Face::Bold);
                g.hit(R::new(a.x, y - 2.0 * s, a.w, lh), Event::Tick, &w);
                y += lh;
            }
        }
        // always drawn: the Models page scrolls (its generic scroll bar) when this block is under the window's bottom edge
        let mut bx = a.x + 4.0 * s;
        let mut acts: Vec<(&str, [u8; 4], char)> = Vec::new();
        if admin {
            acts.push(("List models", ACCENT, 'L'));
            acts.push(("List embedding models", TEXT, 'E'));
        }
        if model.is_some() {
            acts.push(("Options", TEXT, 'o'));
        }
        for (label, col, key) in acts {
            let bw = g.text_width(13.0 * s, label, Face::Bold) + 13.0 * s * 1.7;
            if bx + bw > a.x + a.w - 4.0 * s {
                break;
            }
            bx += g.button(bx, y, 13.0 * s, label, col, Event::Key(Key::Char(key))) + 8.0 * s;
        }
    }
}
