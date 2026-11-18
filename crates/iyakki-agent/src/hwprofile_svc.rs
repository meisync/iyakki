//! The parts of `hwprofile` that need the services of this install (the data and rules are `iyakki_shared::hwprofile`).

use iyakki_shared::hwprofile::{no_computers, *};

/// The one-time card for a computer the install has not seen before: to every administrator named. Returns whether a note was raised.
pub fn announce_new_computer(svc: &crate::services::Services, c: &Computers, admins: &[String]) -> bool {
    if !c.new_computer {
        return false;
    }
    let body = "Its sound, display and keyboard settings are kept separately from your other computers. Settings -> This computer shows and changes that.";
    for a in admins {
        let _ = svc.notify_once(a, crate::inbox::Kind::Device, &format!("New computer: {}", c.me.label), body, Some(crate::inbox::Link::new("settings", "computer")));
    }
    !admins.is_empty()
}

/// Settings state for [`crate::settings::SettingsStore::attach_computers`] at the real start: recognises this computer, overlays its
/// values, and arms the hooks.
pub fn attach_real(svc: &crate::services::Services) -> Option<Computers> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = Computers::open(svc.dir(), &RealFacts, iyakki_clock::now_unix(), st.get());
    st.attach_computers(c.clone());
    activate(&c);
    Some(c)
}

/// The computers of a running install (`None` before the real start attached one, e.g. in a cloud instance without a profile).
pub fn computers_of(svc: &crate::services::Services) -> Option<Computers> {
    svc.settings.lock().unwrap_or_else(|e| e.into_inner()).computers().cloned()
}

/// Share a category between all computers (`shared`) or keep it per computer.
pub fn set_sharing(svc: &crate::services::Services, cat: Category, shared: bool) -> Result<String, String> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = st.computers().cloned().ok_or_else(no_computers)?;
    c.set_shared(cat, shared)?;
    // going back to per computer: this computer's profile takes what the settings hold now
    st.update(|_| {}).map_err(|e| e.to_string())?;
    Ok(if shared { format!("{} is now shared by all computers.", cat.label()) } else { format!("{} is now kept per computer.", cat.label()) })
}

/// Takes another computer's settings over this one's (the categories kept per computer).
pub fn copy_from(svc: &crate::services::Services, name: &str) -> Result<String, String> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = st.computers().cloned().ok_or_else(no_computers)?;
    let other = c.find_other(name)?;
    let mut res = Ok(());
    st.update(|s| res = c.copy_from(&other.id, s)).map_err(|e| e.to_string())?;
    res?;
    Ok(format!("This computer now has the settings of {} (the display size applies at the next sign-in).", other.label))
}

/// Back to the detection defaults for this computer's categories.
pub fn reset(svc: &crate::services::Services) -> Result<String, String> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = st.computers().cloned().ok_or_else(no_computers)?;
    let mut res = Ok(());
    st.update(|s| res = c.reset(s)).map_err(|e| e.to_string())?;
    res?;
    Ok("This computer's sound, display, keyboard, microphone, GPU and power settings are back to automatic.".into())
}

/// Forget another computer: its profile is deleted (it starts fresh if it is used again).
pub fn forget(svc: &crate::services::Services, name: &str) -> Result<String, String> {
    let c = computers_of(svc).ok_or_else(no_computers)?;
    let other = c.find_other(name)?;
    c.forget(&other.id)?;
    Ok(format!("{} is forgotten.", other.label))
}
