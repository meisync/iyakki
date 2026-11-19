//! Chat tools for casting to a TV or speaker (F-MEDIA-11, `upnp_cast.rs`): `cast_renderers` (free: looks for UPnP renderers on the local
//! network for a few seconds) and `cast_play` / `cast_stop` (every call asks: it makes another device on the network play a file of the
//! person's own shared folders from this computer's media server, and the server is switched on only by the person).

use std::net::UdpSocket;
use std::time::Duration;

use serde_json::{json};

use crate::settingstools::{arg_str, no_args, Entry, Env};
use crate::upnp_cast::{self, Wire};

const TAB: &str = "Shares";
pub const TOOLS: [&str; 3] = ["cast_renderers", "cast_play", "cast_stop"];

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
        assert_policy(&tl, &["cast_renderers"], &["cast_play", "cast_stop"]);
        assert!(tl.call("cast_renderers", &json!({})).unwrap().contains("No player"));
        assert!(tl.call("cast_play", &json!({"renderer": "TV", "item": "a1"})).unwrap_err().contains("cast_renderers"));
        assert!(tl.call("cast_stop", &json!({"renderer": "TV"})).is_err());
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
