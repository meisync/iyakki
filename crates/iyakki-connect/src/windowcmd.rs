//! The commands `window` and `devices` (the pieces that need no browser): read the link or name, decide about the certificate, find or make
//! this computer's device key for the instance, and start the loopback window server. `lib.rs` opens the browser and waits.

use std::io::{BufRead, Write};

use iyakki_link::cloudclient::{probe_der, Pin, Target};
use iyakki_link::sigscheme::DeviceKey;

use crate::instances::{self, Instances};
use crate::link::{self, LinkTarget, PinDecision};
use crate::window::{Identity, Options, Persist, Window};
use crate::{devkey, Ctx, Fail};

/// A window ready to be opened.
pub struct Prepared {
    pub window: Window,
    /// The saved name of the instance.
    pub name: String,
    pub browser: Option<String>,
    pub browser_args: Vec<String>,
    /// Print the local address instead of opening a browser.
    pub print_only: bool,
}

fn values<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.iter().enumerate().filter(|(_, a)| *a == name).filter_map(|(i, _)| args.get(i + 1)).map(String::as_str).collect()
}

/// `window LINK-OR-ADDRESS-OR-NAME [--name NAME] [--yes] [--browser PATH] [--browser-arg ARG]... [--port N] [--no-browser]`.
pub fn prepare(args: &[String], ctx: &Ctx, input: &mut dyn BufRead, out: &mut dyn Write) -> Result<Prepared, Fail> {
    let rest = args.get(1..).unwrap_or(&[]);
    let value_opts = ["--name", "--browser", "--browser-arg", "--port"];
    let mut pos = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if value_opts.contains(&rest[i].as_str()) {
            i += 2;
        } else {
            if !rest[i].starts_with("--") {
                pos.push(rest[i].as_str());
            }
            i += 1;
        }
    }
    let [given] = pos.as_slice() else { return Err(Fail::Error(crate::USAGE.into())) };
    let flag = |f: &str| rest.iter().any(|a| a == f);
    let instances = Instances::new(ctx.store.dir());
    let port: u16 = match values(rest, "--port").first() {
        Some(p) => p.parse().map_err(|_| Fail::Error(format!("{p:?} is not a port")))?,
        None => 0,
    };

    // 1. a saved name, or a link, or an address
    let linkish = given.contains("://") || given.contains('#') || given.contains('/');
    let LinkTarget { addr, token, pin: linked } = match (linkish, instances.find(given)?) {
        (false, Some(rec)) => LinkTarget { addr: rec.addr, token: None, pin: None },
        _ => link::parse_link(given)?,
    };
    let saved = instances.find(&addr)?;
    let name = match values(rest, "--name").first() {
        Some(n) => instances::clean_name(n)?,
        None => saved.as_ref().map_or_else(|| instances::default_name(&addr), |r| r.name.clone()),
    };

    // 2. the instance's certificate: a changed one is a hard stop, before any secret
    let shown = pin_of_instance(&addr, ctx)?;
    let stored = ctx.store.get(&addr)?;
    match link::decide_pin(&addr, stored, linked, shown) {
        PinDecision::Use => {}
        PinDecision::Stop(m) => return Err(Fail::PinChanged(m)),
        PinDecision::Remember => {
            ctx.store.set(&addr, shown)?;
            writeln!(out, "pinned {addr} to {} (the link named this certificate)", shown.display()).map_err(|e| Fail::Error(e.to_string()))?;
        }
        PinDecision::Ask => {
            let mut a = vec!["pin".to_string(), addr.clone()];
            if flag("--yes") {
                a.push("--yes".into());
            }
            crate::run(&a, ctx, input, out)?;
        }
    }
    let pin = ctx.store.get(&addr)?.ok_or_else(|| Fail::Error(format!("{addr} is not pinned")))?;

    // 3. this computer's device key for the instance: the saved one, or (with a link) a new one that is kept once the instance has enrolled it
    let identity = match (&token, &saved) {
        (Some(_), _) => Identity { key: DeviceKey::generate()?, device: None, saved: false },
        (None, Some(rec)) if rec.device.is_some() => {
            let key = instances.key(&addr)?.ok_or_else(|| Fail::Error(format!("the key of {name} is missing from {}: use a new link", ctx.store.dir().display())))?;
            Identity { key, device: rec.device.clone(), saved: true }
        }
        _ => return Err(Fail::Error(format!("this computer has no device on {addr} yet: open a one-time link (Settings, Remote access, Connect a client, on the IYAKKI machine) with `iyakki-connect window LINK`"))),
    };
    let window = Window::start(Options { target: Target { addr: addr.clone(), pin }, link_token: token, identity, persist: Some(Persist { instances, name: name.clone() }), port })
        .map_err(Fail::Error)?;
    Ok(Prepared {
        window,
        name,
        browser: values(rest, "--browser").first().map(|s| s.to_string()).or_else(|| std::env::var("IYAKKI_CONNECT_BROWSER").ok()),
        browser_args: values(rest, "--browser-arg").iter().map(|s| s.to_string()).collect(),
        print_only: flag("--no-browser"),
    })
}

fn pin_of_instance(addr: &str, ctx: &Ctx) -> Result<Pin, Fail> {
    let der = probe_der(addr, ctx.timeout)?;
    Ok(iyakki_link::cloudclient::pin_of(&der))
}

/// `devices` lists the instances enrolled on this computer; `devices forget NAME` forgets one.
pub fn devices(rest: &[&str], ctx: &Ctx, out: &mut dyn Write) -> Result<(), Fail> {
    let say = |out: &mut dyn Write, s: String| writeln!(out, "{s}").map_err(|e| Fail::Error(e.to_string()));
    let store = Instances::new(ctx.store.dir());
    match rest {
        [] => {
            let all = store.list()?;
            if all.is_empty() {
                return say(out, "no instance has a device on this computer yet (use `iyakki-connect window LINK`)".into());
            }
            for r in all {
                let key = store.key(&r.addr).ok().flatten().map_or("(key missing)".to_string(), |k| devkey::fingerprint(&k));
                say(out, format!("{}  {}  device {}  key {key}", r.name, r.addr, r.device.as_deref().unwrap_or("(not enrolled)")))?;
            }
            Ok(())
        }
        ["forget", name] => {
            if store.forget(name)? {
                say(out, format!("forgot {name}: its device key is deleted from this computer. The instance still lists the device until you revoke it in Settings, Remote access, Connect a client."))
            } else {
                Err(Fail::Error(format!("{name} is not a saved instance (see `iyakki-connect devices`)")))
            }
        }
        _ => Err(Fail::Error(crate::USAGE.into())),
    }
}
