//! Chat tools for resuming the session after a restart of the shell (F-SHELL-65): developer and test machines only. Reading is free;
//! changing the switch asks the person on a card every time. Everything goes through `cx.core`.

use serde_json::json;

use super::chat_themes::{no_args, Entry, Env};
use super::chat_port::ChatPort;

pub const TOOLS: [&str; 2] = ["session_resume_status", "session_resume_set"];

pub const PROMPT_RULE: &str = "\nRESUME AFTER A RESTART (developer and test machines): session_resume_status says whether the desktop puts the person back without the password after it restarted (off by default; it works only while developer mode is on, for a minute and a half after the shell last said it was there, never after a sign-out or a reboot, and the vault stays locked until the password is typed); session_resume_set switches it on or off and asks the person first.";

pub fn entries(cx: &dyn ChatPort, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let core = cx.core();
        v.push(Entry::read("Developer", "session_resume_status", "Say whether the desktop puts the person back at their open apps without the password after the shell restarted: the Settings switch, and whether developer mode is on (both are needed). The vault stays locked in every case.", no_args(), move |_| {
            let r = core.session_resume();
            Ok(format!("Resume after a restart: switch {}, {}. {}", if r.switch_on() { "on" } else { "off (default)" }, if r.allowed() { "so it is active now (developer mode is on)" } else { "so it is not active (it needs the switch AND developer mode)" }, "A restart puts the person back without the password; the vault stays locked until they type it."))
        }));
    }
    {
        let (core, e1) = (cx.core(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(
            Entry::change("Developer", "session_resume_set", "Switch on or off putting the person back at the desktop without the password after the shell restarted (developer and test machines; it works only while developer mode is on, and the vault stays locked). The person approves a card first.", schema, |a| format!("Resume after a restart: {}", if a["on"].as_bool().unwrap_or(false) { "on" } else { "off" }), move |a| {
                let on = a.get("on").and_then(|x| x.as_bool()).ok_or("on is true or false")?;
                core.session_resume().set_switch(on)?;
                e1.audit("settings.developer", &format!("resume after restart {}", if on { "on" } else { "off" }));
                Ok(format!("Resume after a restart is {}.", if on { "on (active only while developer mode is on)" } else { "off" }))
            })
            .admin(),
        );
    }
    v
}

#[cfg(test)]
#[path = "chat_resume_tests.rs"]
mod tests;
