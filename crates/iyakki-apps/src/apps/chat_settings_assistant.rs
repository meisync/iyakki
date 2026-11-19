//! Chat tools for Settings -> Assistant (docs/development/CHAT-REACHABILITY.md): what the assistant remembers of the work in a workspace
//! (status, the switch, forget one workspace or one chat) and the stall limits (all models, or one provider). They do what the page does.
//! Reading is free; every change asks the person each time. Forgetting always asks, with what goes on the card, and is never granted
//! for a session. The history is the signed-in person's own.

use std::sync::Arc;

use iyakki_memx::history::History;
use iyakki_shared::settings::ProviderLimits;
use iyakki_shared::settingstools::{Entry};
use iyakki_shared::settingstools::{arg_bool, arg_str, no_args};
use super::chat_settings::Env;
use serde_json::{json, Value};

use super::AppCx;

const TAB: &str = "Assistant";

pub const TOOLS: [&str; 5] = ["assistant_memory_status", "assistant_memory_set", "assistant_memory_forget", "assistant_limits_status", "assistant_limits_set"];

pub const PROMPT_RULE: &str = "\nASSISTANT MEMORY AND LIMITS (Settings -> Assistant): assistant_memory_status lists the workspaces the assistant remembers work of and the chats in them, and whether the history is on (free). assistant_memory_set turns the history on or off (asks). assistant_memory_forget forgets one workspace's history, or one chat's entries in it (always asks; it cannot be undone and is never granted for a session). assistant_limits_status shows how long a model may think or stay silent before a call is given up, for all models and per provider (free). assistant_limits_set changes them (asks; administrators).";

fn limit_arg(a: &Value, key: &str) -> Result<Option<u32>, String> {
    match a.get(key) {
        None => Ok(None),
        Some(v) => v.as_u64().filter(|n| *n <= 3600).map(|n| Some(n as u32)).ok_or_else(|| format!("{key} is a number of seconds from 0 (the default) to 3600")),
    }
}

fn words(v: u32) -> String {
    if v == 0 { "default".into() } else { format!("{v} s") }
}

pub fn entries(cx: &AppCx, env: &Env) -> Vec<Entry> {
    let kb = cx.kb().clone();
    let user = cx.session.user.clone();
    let mut v = Vec::new();
    {
        let (e, kb, user) = (env.clone(), kb.clone(), user.clone());
        v.push(Entry::read(TAB, "assistant_memory_status", "Show whether the assistant keeps a history of the work in each workspace, and which workspaces and chats hold entries.", no_args(), move |_| {
            let on = !e.settings.lock().map(|s| s.get().no_workspace_history).unwrap_or(false);
            let mut out = format!("History is {}.", if on { "on" } else { "off (what is already kept stays until it is forgotten)" });
            let ws = History::workspaces_of(&kb, &user);
            if ws.is_empty() {
                out.push_str("\nNo workspace has any history.");
            }
            for w in ws {
                out.push_str(&format!("\n- {}: {} entries from {} chat(s)", w.name, w.entries, w.chats));
                for (chat, n) in History::sessions_of(&kb, &user, &w.name) {
                    out.push_str(&format!("\n    chat {chat}: {n} entries"));
                }
            }
            Ok(out)
        }));
    }
    {
        let e = env.clone();
        v.push(Entry::change(TAB, "assistant_memory_set", "Turn the workspace history on or off (`remember`). Off keeps nothing new; what is kept stays. The person approves a card first.", json!({"type": "object", "properties": {"remember": {"type": "boolean"}}, "required": ["remember"]}), |a| format!("Turn the workspace history {}", if a.get("remember").and_then(Value::as_bool) == Some(true) { "on" } else { "off" }), move |a| {
            let remember = arg_bool(a, "remember")?;
            e.settings.lock().map_err(|_| "the settings are busy".to_string())?.update(|s| s.no_workspace_history = !remember).map_err(|x| x.to_string())?;
            e.audit("settings.assistant.history", if remember { "on" } else { "off" });
            Ok(format!("The workspace history is {}.", if remember { "on" } else { "off" }))
        }));
    }
    {
        let (kb2, user2, kb3, user3) = (kb.clone(), user.clone(), kb.clone(), user.clone());
        let e = env.clone();
        let find = move |kb: &Arc<std::sync::Mutex<iyakki_memx::kb::KnowledgeBase>>, user: &str, a: &Value| -> Result<(String, Option<String>, usize), String> {
            let ws = arg_str(a, "workspace")?;
            let w = History::workspaces_of(kb, user).into_iter().find(|w| w.name == ws).ok_or_else(|| format!("there is no history for the workspace `{ws}`; assistant_memory_status lists them"))?;
            match a.get("chat").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
                None => Ok((w.name, None, w.entries)),
                Some(c) => {
                    let n = History::sessions_of(kb, user, &w.name).into_iter().find(|(s, _)| s == c).map(|x| x.1).ok_or_else(|| format!("chat `{c}` left nothing in `{}`", w.name))?;
                    Ok((w.name, Some(c.to_string()), n))
                }
            }
        };
        let find2 = find.clone();
        let find3 = find.clone();
        v.push(
            Entry::change(TAB, "assistant_memory_forget", "Forget the history of one workspace (`workspace`), or only what one chat left in it (`chat`). Cannot be undone. The person approves a card first, every time.", json!({"type": "object", "properties": {"workspace": {"type": "string"}, "chat": {"type": "string", "description": "A chat id from assistant_memory_status"}}, "required": ["workspace"]}), |a| format!("Forget the history of {}{}", a.get("workspace").and_then(Value::as_str).unwrap_or("?"), a.get("chat").and_then(Value::as_str).map(|c| format!(" (chat {c} only)")).unwrap_or_default()), move |a| {
                let (ws, chat, _) = find(&kb2, &user2, a)?;
                let n = match &chat {
                    Some(c) => History::forget_session_of(&kb2, &user2, &ws, c),
                    None => History::forget_workspace_of(&kb2, &user2, &ws),
                };
                e.audit("settings.assistant.forget", &format!("{ws}{}: {n} entries", chat.map(|c| format!(" chat {c}")).unwrap_or_default()));
                Ok(format!("Forgot {n} entries."))
            })
            .with_preview(move |a| match find2(&kb3, &user3, a) {
                Ok((ws, None, n)) => format!("Forget all {n} entries of the workspace {ws}. This cannot be undone."),
                Ok((ws, Some(c), n)) => format!("Forget the {n} entries chat {c} left in {ws}. This cannot be undone."),
                Err(m) => format!("This would be refused: {m}"),
            })
            .with_refusal({
                let (kb, user) = (kb.clone(), user.clone());
                move |a| find3(&kb, &user, a).err()
            }),
        );
    }
    {
        let e = env.clone();
        let providers = cx.settings().clone();
        v.push(Entry::read(TAB, "assistant_limits_status", "Show how many seconds a model may think without answering, and stay silent, before a call is given up: for all models and for each provider that has its own (0 is the default: 60 and 120).", no_args(), move |_| {
            let st = e.settings.lock().map_err(|_| "the settings are busy".to_string())?;
            let g = st.get();
            let mut out = format!("All models: think {}, silent {}.", words(g.think_limit_secs), words(g.silence_limit_secs));
            for p in &g.providers {
                let l = g.provider_limits.get(&p.id).copied().unwrap_or_default();
                out.push_str(&format!("\n- {}: think {}, silent {}{}", p.id, words(l.think_secs), words(l.silence_secs), if l == ProviderLimits::default() { " (follows all models)" } else { "" }));
            }
            drop(st);
            let _ = &providers;
            Ok(out)
        }));
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        let check = move |e: &Env, a: &Value| -> Result<(Option<String>, Option<u32>, Option<u32>), String> {
            let (t, q) = (limit_arg(a, "think_secs")?, limit_arg(a, "silence_secs")?);
            if t.is_none() && q.is_none() {
                return Err("give think_secs, silence_secs or both".into());
            }
            let p = a.get("provider").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty() && *p != "all").map(str::to_string);
            if let Some(p) = &p {
                if !e.settings.lock().map_err(|_| "the settings are busy".to_string())?.get().providers.iter().any(|x| &x.id == p) {
                    return Err(format!("there is no provider `{p}`; use `all` or an id from assistant_limits_status"));
                }
            }
            Ok((p, t, q))
        };
        let check2 = check.clone();
        v.push(
            Entry::change(TAB, "assistant_limits_set", "Change the stall limits in seconds (0 = the default): `think_secs` (pure thinking) and/or `silence_secs` (no token at all), for `all` models (default) or one `provider`. The person approves a card first. Administrators only.", json!({"type": "object", "properties": {"provider": {"type": "string"}, "think_secs": {"type": "integer"}, "silence_secs": {"type": "integer"}}}), |a| format!("Change the stall limits for {}", a.get("provider").and_then(Value::as_str).unwrap_or("all models")), move |a| {
                let (p, t, q) = check(&e, a)?;
                e.settings
                    .lock()
                    .map_err(|_| "the settings are busy".to_string())?
                    .update(|s| match &p {
                        None => {
                            s.think_limit_secs = t.unwrap_or(s.think_limit_secs);
                            s.silence_limit_secs = q.unwrap_or(s.silence_limit_secs);
                        }
                        Some(p) => {
                            let mut l = s.provider_limits.get(p).copied().unwrap_or_default();
                            l.think_secs = t.unwrap_or(l.think_secs);
                            l.silence_secs = q.unwrap_or(l.silence_secs);
                            if l == ProviderLimits::default() {
                                s.provider_limits.remove(p);
                            } else {
                                s.provider_limits.insert(p.clone(), l);
                            }
                        }
                    })
                    .map_err(|x| x.to_string())?;
                e.audit("settings.assistant.limits", &format!("{}: think {:?} silence {:?}", p.as_deref().unwrap_or("all models"), t, q));
                Ok("Saved.".into())
            })
            .admin()
            .with_refusal(move |a| check2(&e2, a).err()),
        );
    }
    v
}
