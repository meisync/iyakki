//! Chat tools for the one-cable link of developer mode (Settings -> Developer mode -> USB link): `usb_link_check` says what the check
//! list says (the steps, each with what to do when it fails) and `usb_link_watch` is the Replug test (the person unplugs and plugs the cable
//! while it counts; it says whether this computer's port noticed the cable). Both only read: they look at the Thunderbolt/USB4 view, the
//! kernel log and the network list, and change nothing, so no card is asked; they are for administrators like the rest of developer mode.
//! The pairing, the authorise step, the speed test and the link settings are not reachable by chat (they change the machine).
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use serde_json::{json, Value};

use crate::directlink::{cli, Env as LinkEnv};
use crate::settingstools::{no_args, Entry, Env};

const TAB: &str = "Developer";

pub const TOOLS: [&str; 2] = ["usb_link_check", "usb_link_watch"];

// The paragraph for these two tools is part of `settingstools_net::PROMPT_RULE` (the group that wires them).


/// Makes the environment the checks look at (the real machine, or a fake in a test). Called once per tool call.
pub type MakeEnv = Arc<dyn Fn() -> Arc<dyn LinkEnv> + Send + Sync>;

/// The real machine, with the data folder of this installation.
pub fn real_env(env: &Env) -> MakeEnv {
    let dir = env.dir.clone();
    let user = env.user.clone();
    Arc::new(move || {
        let home = dir.join("devmode").join("client").join(&user);
        Arc::new(crate::directlink::real::RealEnv::new(&dir, &home))
    })
}

fn run(make: &MakeEnv, args: &[String]) -> Result<String, String> {
    let link = make();
    let mut text = String::new();
    cli::run_with(link.as_ref(), args, &mut |l| {
        text.push_str(l);
        text.push('\n');
    }, &|_| String::new())?;
    Ok(text.trim_end().to_string())
}

pub fn entries(env: &Env, make: MakeEnv) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (m, e) = (make.clone(), env.clone());
        v.push(
            Entry::read(TAB, "usb_link_check", "Run the check list of the one-cable link to another computer (Settings -> Developer mode -> USB link): each step passes, fails or waits, with what to do when it fails. Changes nothing.", no_args(), move |_| {
                e.audit("settings.usblink.check", "");
                run(&m, &["check".to_string()])
            })
            .admin(),
        );
    }
    {
        let (m, e) = (make, env.clone());
        let schema = json!({"type": "object", "properties": {"seconds": {"type": "integer", "description": "how long to watch, 5 to 120 (default 30)"}}});
        v.push(
            Entry::read(TAB, "usb_link_watch", "The Replug test of the one-cable link: watch this computer's Thunderbolt/USB4 ports while the person unplugs the cable and plugs it back in, and say whether the port noticed it. Tell the person to start unplugging first. Changes nothing.", schema, move |a| {
                let secs = match a.get("seconds") {
                    None | Some(Value::Null) => 30,
                    Some(s) => s.as_u64().filter(|n| (5..=120).contains(n)).ok_or("`seconds` is a whole number from 5 to 120")?,
                };
                e.audit("settings.usblink.watch", &format!("{secs} s"));
                run(&m, &["watch".to_string(), secs.to_string()])
            })
            .admin(),
        );
    }
    v
}

#[cfg(test)]
#[path = "settingstools_usblink_tests.rs"]
mod tests;
