//! Settings -> Models -> Google Gemini (EXPERIMENTAL, optional, off by default; docs/GEMINI-PROVIDER.md). `g` opens the setup (the key goes to
//! the system vault, the tier, the models, whether the assistant and speech-to-text use it), `C` shows the consent card (or takes the yes
//! back), `t` on the Google Gemini row sends one tiny harmless request after the yes. Nothing is sent to Google before the person answers
//! Yes on the card for the tier chosen now; the assistant has tools for the state, the setup and taking the yes back, but none for the
//! key or for the yes (`iyakki_agent::settingstools_gemini`).

use super::*;
use super::apps_port::{store_secret, GeminiPort};
use iyakki_shared::gemini::{ConsentState, Tier};
use iyakki_shared::gemini::{self};

const NO: &str = "No: keep everything on this computer";
const YES: &str = "Yes: allow sending audio and text to Google";

impl SettingsApp {
    /// The state of the signed-in person's yes, in the words of the row's chip.
    pub(super) fn gem_chip(p: &dyn GeminiPort) -> (&'static str, [u8; 4]) {
        match gemini::consent_state(&p.data_dir(), &p.user()) {
            ConsentState::Given { .. } => ("sends to Google: you said yes", OK),
            ConsentState::OtherTier { .. } => ("tier changed: asks again", ERR),
            ConsentState::Missing => ("nothing sent: no yes yet", MUTED),
        }
    }

    /// `g`: the setup card. The key field is empty when one is stored (leave it empty to keep it).
    pub(super) fn gem_open_setup(&mut self, p: &dyn GeminiPort) {
        if !self.admin(p) {
            return;
        }
        let c = gemini::load(&p.data_dir());
        let (chat_on, stt_on) = {
            let g = p.settings_get();
            (g.chat.as_ref().is_some_and(|m| m.provider == gemini::PROVIDER_ID), matches!(g.voice.stt, Some(iyakki_shared::voice::SpeechEngine::Gemini { .. })))
        };
        let have = p.secret_has_ref(gemini::KEY_REF);
        let f = Form::new("Google Gemini (experimental, optional)")
            .with_note(&format!(
                "Off unless you use it. Nothing is sent to Google until you say Yes on the next card. Free key: aistudio.google.com, sign in, Get API key (the free tier has small limits; Google may use what you send to improve its products, according to Google's published terms: check them). Or use your own paid key. The key is kept in the system vault, never in settings or logs. {}",
                gemini::FREE_LIMITS
            ))
            .secret("API key", if have { "leave empty to keep the stored key" } else { "paste the key from aistudio.google.com" })
            .choice("Tier (changes the notice and the advice when the limit is reached)", &["free", "paid"], usize::from(c.tier == Tier::Paid))
            .text("Chat model", &c.chat_model, gemini::DEFAULT_CHAT_MODEL)
            .text("Speech-to-text model", &c.stt_model, gemini::DEFAULT_STT_MODEL)
            .choice("The assistant uses Gemini", &["no", "yes"], usize::from(chat_on))
            .choice("Speech-to-text uses Gemini", &["no", "yes"], usize::from(stt_on));
        self.open_form(FormKind::GeminiSetup, f);
    }

    /// Save the setup. Returns the consent card when the person has not said yes yet for this tier.
    pub(super) fn gem_submit(&mut self, p: &dyn GeminiPort, form: &mut Form) -> Option<Modal> {
        if !self.admin(p) {
            return None;
        }
        let r = self.gem_save(p, form);
        match r {
            Ok(m) => {
                self.ok(m);
                if matches!(gemini::consent_state(&p.data_dir(), &p.user()), ConsentState::Given { .. }) { None } else { Some(Modal::Form(FormKind::GeminiConsent, Self::gem_consent_form(p))) }
            }
            Err(e) => {
                form.error = Some(e);
                Some(Modal::Form(FormKind::GeminiSetup, form.clone()))
            }
        }
    }

    fn gem_save(&mut self, p: &dyn GeminiPort, form: &Form) -> Result<String, String> {
        let key = form.raw(0).trim().to_string();
        if !key.is_empty() {
            if key.len() < 16 || key.chars().any(char::is_whitespace) {
                return Err("That does not look like a Gemini key: copy the whole key again from aistudio.google.com (no spaces).".into());
            }
            store_secret(p, gemini::KEY_NAME, &key, true)?;
        }
        let ch = gemini::Change {
            tier: Some(if form.choice_idx(1) == 1 { Tier::Paid } else { Tier::Free }),
            enabled: Some(true),
            chat_model: Some(form.value(2)).filter(|m| !m.is_empty()).or_else(|| Some(gemini::DEFAULT_CHAT_MODEL.to_string())),
            stt_model: Some(form.value(3)).filter(|m| !m.is_empty()).or_else(|| Some(gemini::DEFAULT_STT_MODEL.to_string())),
            use_chat: Some(form.choice_idx(4) == 1),
            use_stt: Some(form.choice_idx(5) == 1),
        };
        gemini::apply(&p.data_dir(), &p.settings_ref(), &ch)?;
        p.audit("settings.gemini", &format!("setup: tier {}, chat {}, speech {}", ch.tier.map_or("", Tier::label), if ch.use_chat == Some(true) { "on" } else { "off" }, if ch.use_stt == Some(true) { "on" } else { "off" }));
        Ok("Google Gemini is set up. Nothing is sent to Google until you say Yes on the consent card (key C).".into())
    }

    /// The consent card for the tier chosen now: a choice, "No" first, so Enter alone never agrees.
    pub(super) fn gem_consent_form(p: &dyn GeminiPort) -> Form {
        let tier = gemini::load(&p.data_dir()).tier;
        Form::new("Send audio and text to Google?").with_note(&gemini::consent_card(tier)).choice("Your answer (Left/Right to change, Enter to confirm)", &[NO, YES], 0)
    }

    /// `C`: the card when there is no yes for this tier; a question about taking it back when there is.
    pub(super) fn gem_consent_key(&mut self, p: &dyn GeminiPort) {
        match gemini::consent_state(&p.data_dir(), &p.user()) {
            ConsentState::Given { date } => {
                self.modal = Some(Modal::Confirm(ConfirmKind::GeminiRevoke, format!("You said yes on {date}. Take it back? Nothing goes to Google after that.")));
            }
            _ => self.open_form(FormKind::GeminiConsent, Self::gem_consent_form(p)),
        }
    }

    /// The person answered the card. Only a person at the keyboard gets here; no assistant tool leads to it.
    pub(super) fn gem_consent_submit(&mut self, p: &dyn GeminiPort, form: &Form) {
        if form.choice_idx(0) != 1 {
            self.ok("Understood: nothing is sent to Google.");
            return;
        }
        match gemini::grant(&p.data_dir(), &p.user()) {
            Ok(()) => {
                p.audit("settings.gemini", &format!("consent given ({} tier)", gemini::load(&p.data_dir()).tier.label()));
                self.gem_test(p);
            }
            Err(e) => self.fail(e),
        }
    }

    /// Taking the yes back.
    pub(super) fn gem_revoke(&mut self, p: &dyn GeminiPort) {
        match gemini::revoke(&p.data_dir(), &p.user()) {
            Ok(_) => {
                p.audit("settings.gemini", "consent taken back");
                self.ok("Your yes is taken back. Nothing is sent to Google until you say yes again.");
            }
            Err(e) => self.fail(e),
        }
    }

    /// `t` on the Google Gemini row: one tiny harmless request ("Reply with the single word: OK"), only after the yes.
    pub(super) fn gem_test(&mut self, p: &dyn GeminiPort) {
        if gemini::allowed(&p.data_dir(), &p.user()).is_err() {
            return self.gem_consent_key(p);
        }
        let pinger = p.gemini_pinger();
        let c = gemini::load(&p.data_dir());
        self.start("asking Google Gemini", move || {
            JobOut::Msg(match pinger.ping(&c.stt_model, gemini::KEY_REF, c.tier) {
                Ok(t) => Ok(format!("Google Gemini ({}) answered: {t}", c.stt_model)),
                Err(e) => Err(e),
            })
        });
    }
}
