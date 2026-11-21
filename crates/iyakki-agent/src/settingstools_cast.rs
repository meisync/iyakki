//! Chat tools for casting to a TV or speaker (F-MEDIA-11, `upnp_cast.rs`): `cast_renderers` (free: looks for UPnP renderers on the local
//! network for a few seconds) and `cast_play` / `cast_stop` (every call asks: it makes another device on the network play a file of the
//! person's own shared folders from this computer's media server, and the server is switched on only by the person).

use std::net::UdpSocket;
use std::time::Duration;

use serde_json::{json};

use crate::settingstools::{arg_str, no_args, Entry, Env};
use crate::upnp_cast::{self, Wire};

const TAB: &str = "Shares";
pub const TOOLS: [&str; 5] = ["cast_renderers", "cast_play", "cast_stop", "cast_progress", "cast_control"];

pub use iyakki_shared::settingstools_cast::PROMPT_RULE;

fn renderers(wire: &dyn Wire, wait: Duration) -> Result<Vec<upnp_cast::Renderer>, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("cannot search the network: {e}"))?;
    Ok(upnp_cast::discover(wire, &sock, upnp_cast::ssdp_group(), wait, false))
}

pub fn entries(env: &Env) -> Vec<Entry> {
    entries_with(env, || Box::new(upnp_cast::TcpWire), |w| renderers(w, Duration::from_secs(3)))
}

type Finder = fn(&dyn Wire) -> Result<Vec<upnp_cast::Renderer>, String>;

pub fn entries_with(env: &Env, wire: fn() -> Box<dyn Wire>, find: Finder) -> Vec<Entry> {
    let mut v = Vec::new();
    v.push(Entry::read(TAB, "cast_renderers", "Look for TVs and speakers on the local network that can play media from this computer's media server (takes a few seconds).", no_args(), move |_| {
        let w = wire();
        let list = find(w.as_ref())?;
        Ok(if list.is_empty() { "No player that can be sent media was found on the local network.".into() } else { list.iter().map(|r| format!("{} ({})", r.name, r.control.ip())).collect::<Vec<_>>().join("\n") })
    }));
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"renderer": {"type": "string", "description": "A name from cast_renderers."}, "item": {"type": "string", "description": "An item id of the media server, such as a3 or v1."}}, "required": ["renderer", "item"]});
        v.push(Entry::change(TAB, "cast_play", "Make a TV or speaker on the local network play an item of the person's media server. The person approves a card first.", schema, |a| format!("Play {} on {}", a["item"].as_str().unwrap_or("an item"), a["renderer"].as_str().unwrap_or("a player")), move |a| {
            let (name, item) = (arg_str(a, "renderer")?, arg_str(a, "item")?);
            let w = wire();
            let list = find(w.as_ref())?;
            let r = list.iter().find(|r| r.name.eq_ignore_ascii_case(&name)).ok_or("no player of that name was found; call cast_renderers")?;
            let (title, mime, video, url) = crate::upnp_serve::cast_source(&e.user, &item, r.control.ip()).ok_or("the media server is not on, or has no such item (Settings -> Shared folders)")?;
            upnp_cast::cast(w.as_ref(), r, &url, &title, &mime, video)?;
            e.audit("media.cast", &format!("{} to {}", item, r.name));
            Ok(format!("{} is playing {title}.", r.name))
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"renderer": {"type": "string", "description": "A name from cast_renderers."}}, "required": ["renderer"]});
        v.push(Entry::change(TAB, "cast_stop", "Stop what a TV or speaker on the local network is playing. The person approves a card first.", schema, |a| format!("Stop {}", a["renderer"].as_str().unwrap_or("a player")), move |a| {
            let name = arg_str(a, "renderer")?;
            let w = wire();
            let list = find(w.as_ref())?;
            let r = list.iter().find(|r| r.name.eq_ignore_ascii_case(&name)).ok_or("no player of that name was found; call cast_renderers")?;
            upnp_cast::stop(w.as_ref(), r)?;
            e.audit("media.cast", &format!("stopped {}", r.name));
            Ok(format!("{} was told to stop.", r.name))
        }));
    }
    v.push(Entry::read(TAB, "cast_progress", "Say how far a TV or speaker on the local network is with what it plays: playing, paused or stopped, the position and the length (a name from cast_renderers).", json!({"type": "object", "properties": {"renderer": {"type": "string", "description": "A name from cast_renderers."}}, "required": ["renderer"]}), move |a| {
        let name = arg_str(a, "renderer")?;
        let w = wire();
        let list = find(w.as_ref())?;
        let r = list.iter().find(|r| r.name.eq_ignore_ascii_case(&name)).ok_or("no player of that name was found; call cast_renderers")?;
        Ok(format!("{}: {}.", r.name, upnp_cast::progress(w.as_ref(), r)?.line()))
    }));
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"renderer": {"type": "string", "description": "A name from cast_renderers."}, "action": {"type": "string", "description": "pause, resume or seek"}, "seconds": {"type": "integer", "description": "seek only: the place to jump to, in seconds from the start (up to 86399)"}}, "required": ["renderer", "action"]});
        v.push(Entry::change(TAB, "cast_control", "Pause, resume or jump to a place (seek) in what a TV or speaker on the local network is playing. The person approves a card first.", schema, |a| match a["action"].as_str() {
            Some("seek") => format!("Jump to {} s on {}", a["seconds"].as_u64().unwrap_or(0), a["renderer"].as_str().unwrap_or("the player")),
            Some("resume") => format!("Resume playing on {}", a["renderer"].as_str().unwrap_or("the player")),
            _ => format!("Pause {}", a["renderer"].as_str().unwrap_or("the player")),
        }, move |a| {
            let name = arg_str(a, "renderer")?;
            let action = arg_str(a, "action")?;
            // checked before the network is touched
            let secs = match action.as_str() {
                "pause" | "resume" => 0,
                "seek" => a.get("seconds").and_then(serde_json::Value::as_u64).filter(|s| *s <= 86_399).ok_or("seconds is a whole number from 0 to 86399")?,
                _ => return Err("action is pause, resume or seek".into()),
            };
            let w = wire();
            let list = find(w.as_ref())?;
            let r = list.iter().find(|r| r.name.eq_ignore_ascii_case(&name)).ok_or("no player of that name was found; call cast_renderers")?;
            match action.as_str() {
                "pause" => upnp_cast::pause(w.as_ref(), r)?,
                "resume" => upnp_cast::resume(w.as_ref(), r)?,
                _ => upnp_cast::seek(w.as_ref(), r, secs)?,
            }
            e.audit("media.cast", &format!("{action} {}", r.name));
            Ok(format!("{}: {action} done.", r.name))
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};
    use std::net::SocketAddr;

    struct Quiet;
    impl Wire for Quiet {
        fn get(&self, _: SocketAddr, _: &str) -> Result<(u16, String), String> {
            Ok((200, String::new()))
        }
        fn soap(&self, _: SocketAddr, _: &str, _: &str, _: &str) -> Result<(u16, String), String> {
            Ok((200, String::new()))
        }
    }

    #[test]
    fn the_tools_list_free_ask_for_playing_and_stopping_and_never_guess_a_player() {
        let r = rig("cast", false);
        let tl = tools(&r, entries_with(&r.env, || Box::new(Quiet), |_| Ok(vec![])));
        assert_policy(&tl, &["cast_renderers", "cast_progress"], &["cast_play", "cast_stop", "cast_control"]);
        assert!(tl.call("cast_renderers", &json!({})).unwrap().contains("No player"));
        assert!(tl.call("cast_play", &json!({"renderer": "TV", "item": "a1"})).unwrap_err().contains("cast_renderers"));
        assert!(tl.call("cast_stop", &json!({"renderer": "TV"})).is_err());
        assert!(tl.call("cast_progress", &json!({"renderer": "TV"})).unwrap_err().contains("cast_renderers"));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    fn one_tv() -> Result<Vec<upnp_cast::Renderer>, String> {
        let xml = "<root><device><friendlyName>Den TV</friendlyName><UDN>uuid:1</UDN><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/c</controlURL></service></serviceList></device></root>";
        Ok(vec![upnp_cast::parse_description(xml, "http://192.168.1.9:80/d.xml").unwrap()])
    }

    #[test]
    fn progress_is_free_and_control_checks_its_words_before_the_network_and_is_audited() {
        let r = rig("cast2", false);
        let tl = tools(&r, entries_with(&r.env, || Box::new(Quiet), |_| one_tv()));
        assert_eq!(tl.call("cast_progress", &json!({"renderer": "den tv"})).unwrap(), "Den TV: state unknown.");
        assert!(tl.call("cast_control", &json!({"renderer": "Den TV", "action": "seek"})).unwrap_err().contains("seconds"));
        assert!(tl.call("cast_control", &json!({"renderer": "Den TV", "action": "seek", "seconds": 99999})).is_err());
        assert!(tl.call("cast_control", &json!({"renderer": "Den TV", "action": "rewind"})).unwrap_err().contains("pause, resume or seek"));
        assert!(tl.call("cast_control", &json!({"renderer": "Nobody", "action": "pause"})).unwrap_err().contains("cast_renderers"));
        assert_eq!(tl.call("cast_control", &json!({"renderer": "Den TV", "action": "seek", "seconds": 125})).unwrap(), "Den TV: seek done.");
        assert!(tl.call("cast_control", &json!({"renderer": "Den TV", "action": "pause"})).is_ok());
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("media.cast seek Den TV")));
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
