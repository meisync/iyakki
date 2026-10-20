//! A custom command: any program that takes the request on its command line (`{prompt}`) or standard input and prints text
//! (aider, codex, gemini-cli and the like). Everything it prints is the answer. It runs in the workspace folder as an
//! advisor: IYAKKI cannot hold it to a policy, so the files it changed are reported after the run.

use super::{Auth, CustomCfg, Ev, ModelInfo, ModelSource, ExternalAgent, Install, Invocation, Mode, ParseState, Plan};

pub struct Generic {
    cfg: CustomCfg,
}

impl Generic {
    pub fn new(cfg: CustomCfg) -> Generic {
        Generic { cfg }
    }
    fn words(&self) -> Vec<String> {
        crate::wsexec::split_command(&self.cfg.command)
    }
}

impl ExternalAgent for Generic {
    fn id(&self) -> String {
        format!("custom:{}", self.cfg.id)
    }
    fn label(&self) -> String {
        if self.cfg.name.trim().is_empty() { self.cfg.id.clone() } else { self.cfg.name.trim().into() }
    }
    fn program(&self) -> String {
        self.words().into_iter().next().unwrap_or_default()
    }
    fn cfg_key_ref(&self) -> String {
        self.cfg.key_ref.clone()
    }
    fn cfg_model(&self) -> String {
        self.cfg.model.clone()
    }
    fn builtin_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo::new("", "Default (the command's own choice)", "", ModelSource::BuiltIn)]
    }
    fn custom_models(&self) -> Vec<String> {
        self.cfg.custom_models.clone()
    }
    /// The person's own `models_command`, if they gave one.
    fn list_models_argv(&self) -> Option<Vec<String>> {
        let w = crate::wsexec::split_command(&self.cfg.models_command);
        (!w.is_empty()).then_some(w)
    }
    fn mode(&self) -> Mode {
        Mode::Advisor
    }
    fn parse_auth(&self, _o: &crate::wsexec::Out) -> Auth {
        Auth::Unknown
    }
    fn key_env(&self) -> Option<String> {
        Some(self.cfg.key_env.clone()).filter(|k| !k.is_empty())
    }
    fn install(&self) -> Option<Install> {
        None
    }
    fn login(&self) -> Option<Vec<String>> {
        None
    }
    fn login_help(&self) -> String {
        format!("Sign in to {} the way its own documentation says (in the terminal), or give it a key from the vault in Settings → Agents.", self.label())
    }
    fn plain_text(&self) -> bool {
        true
    }

    fn invocation(&self, p: &Plan) -> Invocation {
        let words = self.words();
        let text = if p.system.is_empty() { p.prompt.to_string() } else { format!("{}\n\n---\n{}", p.system, p.prompt) };
        let mut by_arg = false;
        let argv: Vec<String> = words
            .iter()
            .map(|w| {
                if w.contains("{prompt}") {
                    by_arg = true;
                    w.replace("{prompt}", &text)
                } else {
                    w.replace("{model}", if p.model == "default" { "" } else { p.model })
                }
            })
            .collect();
        let env = match (p.key, self.key_env()) {
            (Some(k), Some(name)) => vec![(name, k.to_string())],
            _ => vec![],
        };
        Invocation { argv, env, stdin: (!by_arg).then(|| text.into_bytes()) }
    }

    fn parse_line(&self, line: &str, st: &mut ParseState) -> Vec<Ev> {
        st.partial.push_str(line);
        st.partial.push('\n');
        vec![Ev::Delta(st.partial.clone())]
    }
}
