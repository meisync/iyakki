//! The casting desk behind the Shares page (F-MEDIA-11): the players found last, and the buttons' actions (find, play an item of the person's
//! media server, pause, resume, seek, stop, progress) as plain calls by player NAME, so the page keeps no network types. Everything goes
//! through [`CastNet`]; tests put a fake one (`use_net`), the real one searches with SSDP and talks plain HTTP (`upnp_cast`). Every action is
//! audited as `media.cast` by the caller, exactly as the chat tools are (`settingstools_cast`).

use crate::upnp_cast::{self, Renderer, Wire};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// What the desk needs of the network and of the media server.
pub trait CastNet: Send + Sync {
    /// Players on the local network (a few seconds when real).
    fn find(&self) -> Result<Vec<Renderer>, String>;
    fn wire(&self) -> Box<dyn Wire>;
    /// Title, type, video and the address a player fetches item `id` from; `None` when the server is off or has no such item.
    fn source(&self, user: &str, id: &str, toward: IpAddr) -> Option<(String, String, bool, String)>;
    /// The items to choose from (id, title, video).
    fn items(&self, user: &str) -> Vec<(String, String, bool)>;
}

struct RealNet;

impl CastNet for RealNet {
    fn find(&self) -> Result<Vec<Renderer>, String> {
        let sock = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("cannot search the network: {e}"))?;
        Ok(upnp_cast::discover(&upnp_cast::TcpWire, &sock, upnp_cast::ssdp_group(), Duration::from_secs(3), false))
    }
    fn wire(&self) -> Box<dyn Wire> {
        Box::new(upnp_cast::TcpWire)
    }
    fn source(&self, user: &str, id: &str, toward: IpAddr) -> Option<(String, String, bool, String)> {
        crate::upnp_serve::cast_source(user, id, toward)
    }
    fn items(&self, user: &str) -> Vec<(String, String, bool)> {
        crate::upnp_serve::items_for(user, 200)
    }
}

static NET: Mutex<Option<Arc<dyn CastNet>>> = Mutex::new(None);
static FOUND: OnceLock<Mutex<Vec<Renderer>>> = OnceLock::new();

fn net() -> Arc<dyn CastNet> {
    NET.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Arc::new(RealNet))
}

fn found() -> &'static Mutex<Vec<Renderer>> {
    FOUND.get_or_init(|| Mutex::new(Vec::new()))
}

/// Tests (and a hand-wired build) put their own network; `None` goes back to the real one. Also forgets the players found so far.
pub fn use_net(n: Option<Arc<dyn CastNet>>) {
    *NET.lock().unwrap_or_else(|e| e.into_inner()) = n;
    found().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Look for players; their names (the list is kept for the actions below).
pub fn find() -> Result<Vec<String>, String> {
    let list = net().find()?;
    let names = list.iter().map(|r| r.name.chars().filter(|c| !c.is_control()).collect::<String>()).collect();
    *found().lock().unwrap_or_else(|e| e.into_inner()) = list;
    Ok(names)
}

/// The items of the person's media server to choose from.
pub fn items(user: &str) -> Vec<(String, String, bool)> {
    net().items(user)
}

fn player(name: &str) -> Result<Renderer, String> {
    found().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|r| r.name.chars().filter(|c| !c.is_control()).collect::<String>() == name).cloned().ok_or_else(|| "that player is not in the list: look for players again".to_string())
}

/// Play item `item` of `user`'s media server on `name`.
pub fn play(user: &str, name: &str, item: &str) -> Result<String, String> {
    let r = player(name)?;
    let n = net();
    let (title, mime, video, url) = n.source(user, item, r.control.ip()).ok_or("the media server is not on, or has no such item (Settings -> Shared folders)")?;
    upnp_cast::cast(n.wire().as_ref(), &r, &url, &title, &mime, video)?;
    Ok(format!("{} is playing {title}.", r.name))
}

pub fn stop(name: &str) -> Result<String, String> {
    let r = player(name)?;
    upnp_cast::stop(net().wire().as_ref(), &r)?;
    Ok(format!("{} was told to stop.", r.name))
}

/// What a button did: pause, resume, seek by a signed number of seconds from where the player is now (never before the start), or seek to a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Pause,
    Resume,
    /// Move by this many seconds from the position the player reports.
    By(i64),
    /// Go to this second (0 to 86399).
    To(u64),
}

pub fn control(name: &str, m: Move) -> Result<String, String> {
    let r = player(name)?;
    let n = net();
    let w = n.wire();
    match m {
        Move::Pause => upnp_cast::pause(w.as_ref(), &r)?,
        Move::Resume => upnp_cast::resume(w.as_ref(), &r)?,
        Move::To(s) if s <= 86_399 => upnp_cast::seek(w.as_ref(), &r, s)?,
        Move::To(_) => return Err("a place to jump to is from 0 to 86399 seconds".into()),
        Move::By(d) => {
            let now = upnp_cast::progress(w.as_ref(), &r)?.position_secs.ok_or("the player does not say where it is, so it cannot move from there")?;
            let to = (now as i64 + d).clamp(0, 86_399) as u64;
            upnp_cast::seek(w.as_ref(), &r, to)?;
        }
    }
    Ok(format!("{}: {} done.", r.name, match m { Move::Pause => "pause", Move::Resume => "resume", Move::By(_) | Move::To(_) => "seek" }))
}

/// How far the player is: one line, and the position in seconds when it said one.
pub fn progress(name: &str) -> Result<(String, Option<u64>), String> {
    let r = player(name)?;
    let p = upnp_cast::progress(net().wire().as_ref(), &r)?;
    Ok((format!("{}: {}", r.name, p.line()), p.position_secs))
}

#[cfg(test)]
#[path = "upnp_cast_desk_tests.rs"]
mod tests;
