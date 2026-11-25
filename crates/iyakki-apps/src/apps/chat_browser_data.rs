//! Chat tools for what the browser keeps for a person (Settings -> Browser): bookmark folders, history search, zoom and the saved
//! name and address. They call the same functions as the browser tile (`browser_data::Data`, `browser_forms`), so a change by chat and a
//! change on the page cannot differ. Reads are free, every change asks. The saved name and address are in the vault: a tool says which
//! fields are kept, never what they hold, and nothing here can set one (the person types them in the address bar with `profile set`).

use iyakki_shared::settingstools::{Entry};
use crate::apps::chat_port::ChatPort;
#[cfg(test)]
use crate::apps::chat_port::CxChat;
use iyakki_shared::settingstools::{arg_str, no_args};
use super::chat_settings::Env;
use serde_json::json;

use super::browser_data as bd;
use super::browser_forms as bf;
#[cfg(test)]
use super::AppCx;

pub const TOOLS: [&str; 28] = ["browser_install_site", "site_apps_list", "site_app_open", "site_app_remove", "browser_data_export", "browser_data_import", "browser_data_to_tenant", "browser_data_from_tenant", "browser_protection_choices", "browser_protection_choices_set", "browser_bookmarks", "browser_bookmark_folder", "browser_history_search", "browser_zoom_status", "browser_zoom_set", "browser_profile_status", "browser_profile_clear", "browser_thumbnails_status", "browser_thumbnails_set", "browser_downloads_list", "browser_download_open", "browser_reports_list", "browser_reports_clear", "browser_media_sites", "browser_media_site_forget", "browser_av_addon_status", "browser_media_policy_status", "browser_media_policy_set"];

pub const PROMPT_RULE: &str = "\nBROWSER DATA: browser_data_export copies the sealed browser data to a file for another computer and browser_data_import takes one in (both ask; the key is not in the file); browser_data_to_tenant and browser_data_from_tenant move the sealed data to a cloud tenant you are signed in to and back (both ask; the tenant cannot read it; the key stays in your vault); browser_protection_choices and browser_protection_choices_set read and change what is kept of the protected browser data (checkpoint, lock screen, sites left out); browser_bookmarks lists folders (and one folder's bookmarks), browser_history_search finds visited pages by words (titles and sites only), browser_zoom_status lists zoom per site, browser_profile_status says which name/address fields are kept in the vault, never their values, browser_thumbnails_status says whether small pictures of often-visited pages are kept for the start page (all free). browser_bookmark_folder (new, move, rename, delete), browser_zoom_set, browser_profile_clear and browser_thumbnails_set (on, off, clear) ask first. You cannot set or read a saved name or address: the person types `profile set FIELD VALUE` in the browser's address bar. BROWSER DOWNLOADS: browser_downloads_list lists the saved files and which tile opens each (free); browser_download_open opens one saved file by its name in the tile that suits it (EDIT, DATA or MEDIA) and keeps the current screen aside so Back returns (asks first). BROWSER SITE REPORTS: browser_reports_list shows the reports the person saved with the Report site button or the ! key (time, site, status, engine, how many notes; free); browser_reports_clear deletes them (asks first). Reports stay on this computer; nothing is sent. browser_media_sites lists the sites the person allowed or blocked for camera, microphone and screen (free); browser_media_site_forget makes the browser ask again for one site (asks first; it never allows anything). browser_av_addon_status says whether the audio and video add-on (H.264, AAC) is installed and where an administrator installs it (free; installing is in Store, Setup and asks). browser_media_policy_status shows the tallest video the web engine is asked for and whether it may use the add-on's decoders (free); browser_media_policy_set changes either (asks first). SITE APPS: browser_install_site (asks), site_apps_list (free), site_app_open (asks), site_app_remove (asks) make, list, open and delete a site app; never offer a way round a missing decryption module.";

fn site_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

pub fn entries(cx: &dyn ChatPort, env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "site_apps_list", "List the sites installed as app tiles: name, start address, the origins each may load, what it holds, and a note when the site needs the audio and video add-on or a content decryption module that this computer does not have.", no_args(), move |_| {
            let st = super::site_apps::load(&*c);
            if st.apps.is_empty() {
                return Ok("No site is installed as an app.".into());
            }
            let addon = crate::agent().avaddon().detect(c.data_dir());
            Ok(st.apps.iter().map(|a| {
                let mut line = format!("{} ({}); origins: {}; holds: {}", a.name, a.start_url, a.origins.join(", "), a.capabilities().join("; "));
                if let Some(t) = super::site_apps::codec_card(a, &addon) {
                    line.push_str(&format!("; NOTE: {t}"));
                }
                if let Some(t) = super::site_apps::protection_message(&super::site_apps::host_of(&a.start_url), None) {
                    line.push_str(&format!("; NOTE: {t}"));
                }
                // the size of its own cookies and storage (F-BROW-100): a note once it passes the cap, never a deletion
                let size = super::site_apps::profile_size(&super::site_apps::profile_dir(c.data_dir(), &super::browser_config::who(&*c), &a.id));
                if let Some(t) = super::site_apps::size_note(size) {
                    line.push_str(&format!("; NOTE: {t}"));
                }
                line
            }).collect::<Vec<_>>().join("\n"))
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"url": {"type": "string", "description": "The https address of the page to install."}, "name": {"type": "string", "description": "The tile name; leave out to use the page's own."}, "title": {"type": "string", "description": "The page title."}, "manifest": {"type": "string", "description": "The text of the page's web app manifest, if known."}, "favicon": {"type": "string", "description": "The page's icon address, if known."}, "extra_origins": {"type": "array", "items": {"type": "string"}, "description": "More https origins the tile may load (a sign-in host, for example)."}}, "required": ["url"]});
        v.push(Entry::change("Browser", "browser_install_site", "Install a site as an app tile with its own window and its own cookies and storage; other hosts open in the Browser tile. The person approves a card first.", schema, |a| format!("Install {} as an app tile", a["url"].as_str().unwrap_or("a site")), move |a| {
            let page = super::site_apps::PageInfo { url: arg_str(a, "url")?, title: a["title"].as_str().unwrap_or("").to_string(), manifest: a["manifest"].as_str().map(str::to_string), manifest_url: None, favicon: a["favicon"].as_str().map(str::to_string) };
            let extra: Vec<String> = a["extra_origins"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).take(8).collect()).unwrap_or_default();
            let app = super::site_apps::from_page(&page, a["name"].as_str(), &extra)?;
            let mut st = super::site_apps::load(&*c);
            st.add(app.clone())?;
            super::site_apps::save(&*c, &st);
            e1.audit("browser.siteapp", &app.audit("installed"));
            let mut msg = format!("{} is now an app tile ({}).", app.name, app.origins.join(", "));
            if let Some(t) = super::site_apps::codec_card(&app, &crate::agent().avaddon().detect(c.data_dir())) {
                msg.push(' ');
                msg.push_str(t);
            }
            if let Some(t) = super::site_apps::protection_message(&super::site_apps::host_of(&app.start_url), None) {
                msg.push(' ');
                msg.push_str(t);
            }
            Ok(msg)
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"name": {"type": "string", "description": "The site app's name from site_apps_list."}}, "required": ["name"]});
        v.push(Entry::change("Browser", "site_app_open", "Open a site app in a tab of its own in the Browser, with the site's own profile and only its origins (the screen the person is on is kept aside so Back returns). The person approves a card first that names the site and what the tile may hold.", schema, |a| format!("Open the site app {}", a["name"].as_str().unwrap_or("")), move |a| {
            let st = super::site_apps::load(&*c);
            let app = st.find(&arg_str(a, "name")?).cloned().ok_or("no site app has that name; call site_apps_list")?;
            super::download_open::queue_chat(&c.user(), "browser", &format!("site:{}", app.id));
            e1.audit("browser.siteapp", &app.audit("open requested"));
            Ok(format!("Opening {} in the Browser.", app.name))
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"name": {"type": "string", "description": "The site app's name from site_apps_list."}}, "required": ["name"]});
        v.push(Entry::change("Browser", "site_app_remove", "Remove a site app and delete its own cookies, storage and sign-ins. The person approves a card first.", schema, |a| format!("Remove the site app {} and forget its sign-ins", a["name"].as_str().unwrap_or("")), move |a| {
            let mut st = super::site_apps::load(&*c);
            let app = st.find(&arg_str(a, "name")?).cloned().ok_or("no site app has that name; call site_apps_list")?;
            let gone = st.remove(c.data_dir(), &super::browser_config::who(&*c), &app.id)?;
            super::site_apps::save(&*c, &st);
            e1.audit("browser.siteapp", &app.audit("removed"));
            Ok(format!("{} is removed{}.", app.name, if gone { " and its sign-ins are deleted" } else { "" }))
        }));
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_media_policy_status", "Say how tall a video the browser's web engine is asked for (Automatic or a number of pixels) and whether it may use the audio and video add-on's decoders.", no_args(), move |_| {
            let b = super::browser_config::load(&*c);
            let held = c.agent().av_build().codecs_policy(&c.user()) == Some(false);
            Ok(format!("Video quality: {}. Add-on decoders: {}{}.", super::browser_config::cap_words(b.video_max_height), if b.codecs && !held { "allowed" } else { "off" }, if held { " (the tenant's administrator has turned them off)" } else { "" }))
        }));
    }
    {
        let (c, e) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"max_height": {"type": "integer", "description": "The tallest video in pixels: 0 (Automatic), 1080, 720, 480 or 360"}, "codecs": {"type": "boolean", "description": "Allow the audio and video add-on's decoders"}}});
        v.push(Entry::change("Browser", "browser_media_policy_set", "Change the tallest video the web engine is asked for (max_height: 0, 1080, 720, 480 or 360) and/or whether it may use the audio and video add-on's decoders (codecs). Applies to the next page. The person approves a card first.", schema, |a| format!("Browser video: {}{}", a["max_height"].as_u64().map(|h| format!("tallest {}", if h == 0 { "automatic".to_string() } else { h.to_string() })).unwrap_or_default(), a["codecs"].as_bool().map(|b| format!(" decoders {}", if b { "on" } else { "off" })).unwrap_or_default()), move |a| {
            let mut b = super::browser_config::load(&*c);
            if let Some(h) = a["max_height"].as_u64() {
                let h = u32::try_from(h).unwrap_or(u32::MAX);
                if !super::browser_config::VIDEO_CAPS.contains(&h) {
                    return Err("max_height is 0 (Automatic), 1080, 720, 480 or 360".into());
                }
                b.video_max_height = h;
            }
            if let Some(on) = a["codecs"].as_bool() {
                b.codecs = on;
            }
            if a["max_height"].is_null() && a["codecs"].is_null() {
                return Err("say max_height, codecs or both".into());
            }
            super::browser_config::save(&*c, &b);
            e.audit("settings.browser", &format!("Video quality: {}; add-on decoders {}.", super::browser_config::cap_words(b.video_max_height), if b.codecs { "allowed" } else { "off" }));
            Ok(format!("Video quality: {}. Add-on decoders: {}. It applies to the next page.", super::browser_config::cap_words(b.video_max_height), if b.codecs { "allowed" } else { "off" }))
        }));
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_hw_decode_status", "Say whether the web engine decodes video on the graphics chip (VA-API) on this computer: on or off. It is a setting of this computer that an administrator changes.", no_args(), move |_| {
            Ok(format!("Hardware video decoding: {}. It applies the next time the web engine starts, and the engine falls back to the processor by itself when the chip or its driver is missing.", super::browser_tuning::words(super::browser_tuning::load(&*c))))
        }));
    }
    {
        let c = cx.shared();
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean", "description": "true to decode video on the graphics chip, false for the processor"}}, "required": ["on"]});
        v.push(Entry::change("Browser", "browser_hw_decode_set", "Turn hardware video decoding (VA-API) of the web engine on or off for this computer. Only an administrator may. It applies the next time the web engine starts. The person approves a card first.", schema, |a| format!("Hardware video decoding {}", if a["on"].as_bool() == Some(true) { "on" } else { "off" }), move |a| {
            let on = a["on"].as_bool().ok_or("say on: true or false")?;
            super::browser_tuning::set(&*c, on)?;
            Ok(format!("Hardware video decoding: {}. It applies the next time the web engine starts.", super::browser_tuning::words(on)))
        }));
    }
    {
        let data = cx.data_dir().to_path_buf();
        v.push(Entry::read("Browser", "browser_av_addon_status", "Say whether the browser's audio and video add-on (H.264 video, AAC and MP3 sound) is installed on this computer and where an administrator installs it.", no_args(), move |_| {
            let (t, h) = iyakki_shared::avaddon::settings_text(&crate::agent().avaddon().detect(&data));
            Ok(format!("{t}. {h}"))
        }));
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_media_sites", "List the sites the person allowed or blocked for the camera, the microphone or the screen share (site and what was decided).", no_args(), move |_| {
            let m = super::browser_media::load(&*c);
            let l = m.list();
            Ok(if l.is_empty() { "No site is remembered: every request asks.".to_string() } else { l.iter().map(|(h, w)| { let n = m.device_note(h); if n.is_empty() { format!("{h}: {w}") } else { format!("{h}: {w} ({n})") } }).collect::<Vec<_>>().join("\n") })
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"site": {"type": "string", "description": "A site from browser_media_sites, for example meet.example.org."}}, "required": ["site"]});
        v.push(Entry::change("Browser", "browser_media_site_forget", "Make the browser ask again about camera, microphone and screen for one remembered site (it forgets the allow or block and the device chosen for it). The person approves a card first.", schema, |a| format!("Forget what the browser remembers about {} for camera and microphone", a["site"].as_str().unwrap_or("a site")), move |a| {
            let site = arg_str(a, "site")?.to_ascii_lowercase();
            if !site_ok(&site) {
                return Err("that is not a site name".into());
            }
            let mut m = super::browser_media::load(&*c);
            if !m.list().iter().any(|(h, _)| *h == site) {
                return Err(format!("{site} is not remembered; call browser_media_sites"));
            }
            m.revoke(&site);
            super::browser_media::save(&*c, &m);
            e1.audit("settings.browser.media", &format!("{site} forgotten for camera, microphone and screen"));
            Ok(format!("The browser will ask again before {site} uses the camera, the microphone or the screen."))
        }));
    }
    {
        let c = cx.shared();
        let schema = json!({"type": "object", "properties": {"folder": {"type": "string", "description": "A folder name; leave out to list the folders."}}});
        v.push(Entry::read("Browser", "browser_bookmarks", "List the browser's bookmark folders with counts, or the bookmarks (title, site) of one folder (\"-\" is the top level).", schema, move |a| {
            let d = bd::load(&*c);
            if let Some(f) = a.get("folder").and_then(|f| f.as_str()) {
                let f = if f.trim() == "-" { String::new() } else { bd::Data::clean_folder(f) };
                let items: Vec<String> = d.bookmarks.iter().filter(|b| b.folder == f).take(100).map(|b| format!("{} ({})", b.title, bd::host_of(&b.url))).collect();
                return Ok(if items.is_empty() { "That folder holds no bookmarks.".into() } else { items.join("\n") });
            }
            let top = d.bookmarks.iter().filter(|b| b.folder.is_empty()).count();
            let mut out = vec![format!("(top level): {top}")];
            out.extend(d.folder_names().iter().map(|n| format!("{n}: {}", d.bookmarks.iter().filter(|b| &b.folder == n).count())));
            Ok(out.join("\n"))
        }));
    }
    {
        let c = cx.shared();
        let e1 = env.clone();
        let schema = json!({"type": "object", "properties": {"action": {"type": "string", "enum": ["new", "move", "rename", "delete"]}, "name": {"type": "string", "description": "The folder (for move: the folder to put the bookmark in, \"-\" for none)."}, "to": {"type": "string", "description": "rename: the new name."}, "site": {"type": "string", "description": "move: the site (host) of the bookmark."}}, "required": ["action", "name"]});
        v.push(
            Entry::change("Browser", "browser_bookmark_folder", "Make, rename or delete a bookmark folder, or move the bookmark of a site into one (deleting keeps the bookmarks). The person approves a card first.", schema, |a| format!("Bookmarks: {} {}", a["action"].as_str().unwrap_or("change"), a["name"].as_str().unwrap_or("a folder")), move |a| {
                let (action, name) = (arg_str(a, "action")?, arg_str(a, "name")?);
                let mut d = bd::load(&*c);
                let msg = match action.as_str() {
                    "new" => format!("Folder {} made.", d.new_folder(&name)?),
                    "rename" => format!("Renamed ({} bookmarks).", d.rename_folder(&name, &arg_str(a, "to")?)?),
                    "delete" => format!("Folder deleted; its {} bookmarks are at the top level.", d.delete_folder(&name)?),
                    "move" => {
                        let site = arg_str(a, "site")?;
                        let urls: Vec<String> = d.bookmarks.iter().filter(|b| bd::host_of(&b.url) == site).map(|b| b.url.clone()).collect();
                        if urls.is_empty() {
                            return Err(format!("No bookmark for {site}"));
                        }
                        for u in &urls {
                            d.move_bookmark(u, &name)?;
                        }
                        format!("Moved {} bookmark(s) of {site} to {}.", urls.len(), if name == "-" { "the top level" } else { &name })
                    }
                    _ => return Err("action is new, move, rename or delete".into()),
                };
                bd::save(&*c, &d);
                e1.audit("settings.browser", &format!("Bookmark folders: {action}"));
                Ok(msg)
            })
            .with_refusal(|a| (a["action"].as_str() == Some("rename") && a["to"].as_str().is_none_or(|t| t.trim().is_empty())).then(|| "rename needs `to`".to_string())),
        );
    }
    {
        let c = cx.shared();
        let schema = json!({"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "description": "At most 20."}}, "required": ["query"]});
        v.push(Entry::read("Browser", "browser_history_search", "Find pages the person visited: every word must be in the title or address. Returns title, site and when; never full addresses.", schema, move |a| {
            let q = arg_str(a, "query")?;
            let n = a.get("limit").and_then(|l| l.as_u64()).unwrap_or(10).clamp(1, 20) as usize;
            let d = bd::load(&*c);
            let found = d.search_history(&q, n);
            if found.is_empty() {
                return Ok("Nothing in the history matches.".into());
            }
            Ok(found.iter().map(|h| format!("{} ({}) {} visit(s)", if h.title.is_empty() { "(no title)" } else { &h.title }, bd::host_of(&h.url), h.visits)).collect::<Vec<_>>().join("\n"))
        }));
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_zoom_status", "List the sites whose zoom is not 100% and whether the zoom changes only the text.", no_args(), move |_| {
            let d = bd::load(&*c);
            if d.zoom.is_empty() && d.text_zoom.is_empty() {
                return Ok("Every site is at 100%.".into());
            }
            Ok(d.zoom.iter().map(|(h, z)| format!("{h}: {}%{}", (z * 100.0).round(), if d.is_text_zoom(h) { " (text only)" } else { "" })).collect::<Vec<_>>().join("\n"))
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"site": {"type": "string", "description": "The host name, e.g. example.org."}, "percent": {"type": "integer", "description": "50 to 300; 100 resets."}, "text_only": {"type": "boolean", "description": "Make only the letters bigger."}}, "required": ["site", "percent"]});
        v.push(
            Entry::change("Browser", "browser_zoom_set", "Set the zoom of one site (50 to 300 percent, 100 resets), optionally for the text only. The person approves a card first.", schema, |a| format!("Zoom {} to {}%", a["site"].as_str().unwrap_or("a site"), a["percent"]), move |a| {
                let site = arg_str(a, "site")?.to_ascii_lowercase();
                let pct = a.get("percent").and_then(|p| p.as_i64()).ok_or("percent is a number from 50 to 300")?;
                let mut d = bd::load(&*c);
                d.set_zoom(&site, pct as f32 / 100.0);
                if let Some(t) = a.get("text_only").and_then(|t| t.as_bool()) {
                    if t != d.is_text_zoom(&site) {
                        d.toggle_text_zoom(&site);
                    }
                }
                bd::save(&*c, &d);
                e1.audit("settings.browser", &format!("Zoom {site} {pct}%"));
                Ok(format!("{site} is at {pct}%{}.", if d.is_text_zoom(&site) { " (text only)" } else { "" }))
            })
            .with_refusal(|a| {
                let site = a["site"].as_str().unwrap_or("");
                let pct = a["percent"].as_i64().unwrap_or(0);
                if !site_ok(site) {
                    return Some("site is a host name such as example.org".into());
                }
                (!(50..=300).contains(&pct)).then(|| "percent is from 50 to 300".to_string())
            }),
        );
    }
    {
        let (data, user, agent) = (cx.data_dir().to_path_buf(), env.user.clone(), cx.agent());
        v.push(Entry::read("Browser", "browser_protection_choices", "Say what the person chose for their protected browser data: the timed checkpoint, sealing when the screen locks, what is left out at sign-out and the sites never kept. Also says whether protection is on.", no_args(), move |_| {
            let st = agent.profile_store().status(&data, &user);
            Ok(format!("Protection: {}\n{}", st.text(), iyakki_shared::profilepolicy::Policy::load(&data, &user).summary()))
        }));
    }
    {
        // moving the sealed file to another machine (F-VAULT-11): the file stays sealed; the key stays in the person's own vault
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"path": {"type": "string", "description": "A new file to write (in a folder this account may write), for example on a USB stick."}}, "required": ["path"]});
        v.push(Entry::change("Browser", "browser_data_export", "Copy the protected browser data, still sealed, to a new file (for example on a USB stick) so it can be taken to another computer. The cookies stay unreadable without the browser key in the person's vault; the key is not copied. Only while the data is sealed (signed out of the browser, not open). The person approves a card first.", schema, |a| format!("Copy the sealed browser data to {}", a["path"].as_str().unwrap_or("?")), move |a| {
            let p = arg_str(a, "path")?;
            let path = std::path::Path::new(&p);
            if !path.is_absolute() || p.contains("..") || !c.policy().allows_write(&c.user(), &p) {
                return Err("that is not a place this account may write (give a full path in your own folders)".into());
            }
            let n = c.agent().profile_store().export_sealed(c.data_dir(), &c.user(), path, iyakki_schema::now_secs())?;
            e1.audit("settings.browser.data.export", &format!("{n} bytes"));
            Ok(format!("The sealed browser data ({n} bytes) is in {p}. It opens only with the browser key in your own vault (it is not in the file)."))
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"path": {"type": "string", "description": "A sealed browser data file made by browser_data_export."}}, "required": ["path"]});
        v.push(Entry::change("Browser", "browser_data_import", "Take in a sealed browser data file made on another computer as this account's protected browser data. It is checked with this account's own browser key first; the data that was here becomes the backup copy. Refused while the browser data is open. The person approves a card first.", schema, |a| format!("Take in sealed browser data from {}", a["path"].as_str().unwrap_or("?")), move |a| {
            let p = arg_str(a, "path")?;
            let path = std::path::Path::new(&p);
            if !path.is_absolute() || p.contains("..") || !c.policy().allows_read(&c.user(), &p) {
                return Err("that is not a file this account may read (give a full path in your own folders)".into());
            }
            let key = c.vault().user_get(&c.user(), iyakki_shared::profilestore::VAULT_SECRET).ok().ok_or("this account's vault has no browser key, so the file cannot be opened here (the key is not moved by the file)")?;
            c.agent().profile_store().import_sealed(c.data_dir(), &c.user(), path, &key, iyakki_schema::now_secs())?;
            e1.audit("settings.browser.data.import", "sealed file taken in");
            Ok("The sealed browser data was checked with your browser key and taken in; it opens at your next sign-in. The data that was here is kept as the backup copy.".into())
        }));
    }
    {
        // moving the sealed file to a signed-in cloud tenant and back (F-VAULT-11): the tenant holds bytes it cannot read
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"tenant": {"type": "string", "description": "The name of a cloud tenant you are signed in to (see the cloud strip)."}}, "required": ["tenant"]});
        v.push(Entry::change("Browser", "browser_data_to_tenant", "Send the protected browser data, still sealed, to a cloud tenant you are signed in to, so the browser there can use your sign-ins. The tenant cannot read it: the key stays in your own vault and is not sent. Only while the data is sealed here (signed out of the browser). The person approves a card first.", schema, |a| format!("Send the sealed browser data to the tenant {}", a["tenant"].as_str().unwrap_or("?")), move |a| {
            let t = arg_str(a, "tenant")?;
            let n = c.agent().profile_store().send_to_tenant(c.data_dir(), &c.user(), &t, iyakki_schema::now_secs())?;
            e1.audit("settings.browser.data.to_tenant", &format!("{t}: {n} bytes"));
            Ok(format!("The sealed browser data ({n} bytes) is on the tenant {t}. It opens only with the browser key in your own vault."))
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"tenant": {"type": "string", "description": "The name of a cloud tenant you are signed in to."}}, "required": ["tenant"]});
        v.push(Entry::change("Browser", "browser_data_from_tenant", "Bring back the sealed browser data a cloud tenant holds for this account as the protected browser data here. It is checked with this account's own browser key first; the data that was here becomes the backup copy. Refused while the browser data is open. The person approves a card first.", schema, |a| format!("Take in the sealed browser data from the tenant {}", a["tenant"].as_str().unwrap_or("?")), move |a| {
            let t = arg_str(a, "tenant")?;
            let key = c.vault().user_get(&c.user(), iyakki_shared::profilestore::VAULT_SECRET).ok().ok_or("this account's vault has no browser key, so the data cannot be opened here (the key is not moved by the data)")?;
            c.agent().profile_store().fetch_from_tenant(c.data_dir(), &c.user(), &t, &key, iyakki_schema::now_secs())?;
            e1.audit("settings.browser.data.from_tenant", &t);
            Ok("The sealed browser data was checked with your browser key and taken in; it opens at your next sign-in. The data that was here is kept as the backup copy.".into())
        }));
    }
    {
        let (data, user, e1, agent) = (cx.data_dir().to_path_buf(), env.user.clone(), env.clone(), cx.agent());
        let sites = json!({"type": "array", "items": {"type": "string"}, "description": "Site names such as example.com; a site covers its sub-domains. The whole list is replaced."});
        let schema = json!({"type": "object", "properties": {
            "checkpoint_minutes": {"type": "integer", "description": "Minutes between write-backs while the browser data is open and changes; 0 switches it off."},
            "lock_seal": {"type": "boolean", "description": "Seal the browser data when the screen locks and open it again at unlock."},
            "clear_cookies_on_exit": {"type": "boolean", "description": "Leave every cookie out of the sealed file at sign-out."},
            "clear_sites_on_exit": sites.clone(),
            "never_persist": sites}});
        v.push(Entry::change("Browser", "browser_protection_choices_set", "Change what is kept of the protected browser data: the checkpoint interval, sealing at the lock screen, cookies and sites left out at sign-out, sites never kept. Only the fields given change. The person approves a card first. Cookies cannot be picked per site (all or none).", schema, |a| {
            let mut parts = Vec::new();
            for k in ["checkpoint_minutes", "lock_seal", "clear_cookies_on_exit", "clear_sites_on_exit", "never_persist"] {
                if let Some(x) = a.get(k) {
                    parts.push(format!("{k} = {x}"));
                }
            }
            format!("Browser data choices: {}", if parts.is_empty() { "no change".to_string() } else { parts.join(", ") })
        }, move |a| {
            let mut p = iyakki_shared::profilepolicy::Policy::load(&data, &user);
            let sites = |k: &str| -> Result<Option<Vec<String>>, String> {
                match a.get(k) {
                    None => Ok(None),
                    Some(v) => {
                        let arr = v.as_array().ok_or(format!("{k} is a list of site names"))?;
                        if arr.len() > iyakki_shared::profilepolicy::MAX_SITES {
                            return Err(format!("{k} holds at most {} sites", iyakki_shared::profilepolicy::MAX_SITES));
                        }
                        arr.iter().map(|s| iyakki_shared::profilepolicy::clean_site(s.as_str().unwrap_or(""))).collect::<Result<Vec<_>, _>>().map(Some)
                    }
                }
            };
            if let Some(n) = a.get("checkpoint_minutes") {
                p.checkpoint_minutes = n.as_u64().filter(|n| *n <= 24 * 60).ok_or("checkpoint_minutes is a number of minutes from 0 to 1440")? as u32;
            }
            if let Some(b) = a.get("lock_seal") {
                p.lock_seal = b.as_bool().ok_or("lock_seal is true or false")?;
            }
            if let Some(b) = a.get("clear_cookies_on_exit") {
                p.clear_cookies_on_exit = b.as_bool().ok_or("clear_cookies_on_exit is true or false")?;
            }
            if let Some(l) = sites("clear_sites_on_exit")? {
                p.clear_sites_on_exit = l;
            }
            if let Some(l) = sites("never_persist")? {
                p.never_persist = l;
            }
            p.save(&data, &user)?;
            let p = iyakki_shared::profilepolicy::Policy::load(&data, &user);
            e1.audit("settings.browser.protection", "choices changed");
            let on = agent.profile_store().is_protected(&data, &user);
            Ok(format!("Saved.\n{}{}", p.summary(), if on { "" } else { "\nProtect browser data is off, so none of this applies until it is switched on (Settings -> Browser, key P)." }))
        }));
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_profile_status", "Say which saved name and address fields are kept in the vault (field names only, never their values).", no_args(), move |_| {
            let p = bf::load(&*c)?;
            if p.is_empty() {
                return Ok("Nothing is kept.".into());
            }
            Ok(format!("Kept: {}.", bf::ALL.iter().filter(|f| !p.get(**f).is_empty()).map(|f| f.key()).collect::<Vec<_>>().join(", ")))
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        v.push(Entry::change("Browser", "browser_profile_clear", "Remove the saved name and address from the vault. The person approves a card first.", no_args(), |_| "Remove the saved name and address".to_string(), move |_| {
            bf::save(&*c, &bf::Profile::default())?;
            e1.audit("settings.browser", "Saved name and address removed");
            Ok("The saved name and address were removed from the vault.".into())
        }));
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_thumbnails_status", "Say whether small pictures of often-visited pages are kept for the browser's start page, and how many.", no_args(), move |_| {
            let d = bd::load(&*c);
            let n = super::browser_thumbs::Thumbs::new(&thumbs_dir(&*c)).count();
            Ok(format!("Pictures of often-visited pages are {}; {n} kept.", if d.thumbs_off { "off" } else { "on" }))
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"mode": {"type": "string", "enum": ["on", "off", "clear"]}}, "required": ["mode"]});
        v.push(Entry::change("Browser", "browser_thumbnails_set", "Switch the small pictures of often-visited pages on or off (off deletes them), or delete them. The person approves a card first.", schema, |a| format!("Browser pictures: {}", a["mode"].as_str().unwrap_or("change")), move |a| {
            let mode = arg_str(a, "mode")?;
            let mut d = bd::load(&*c);
            let t = super::browser_thumbs::Thumbs::new(&thumbs_dir(&*c));
            match mode.as_str() {
                "on" => d.thumbs_off = false,
                "off" => {
                    d.thumbs_off = true;
                    t.clear();
                }
                "clear" => t.clear(),
                _ => return Err("mode is on, off or clear".into()),
            }
            bd::save(&*c, &d);
            e1.audit("settings.browser", &format!("Page pictures: {mode}"));
            Ok(format!("Pictures of often-visited pages: {mode}."))
        }));
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_downloads_list", "List the files the browser saved to Downloads (newest first) with their size and the tile that opens each: text and code in EDIT, tables in DATA, music and video in MEDIA.", no_args(), move |_| {
            let l = super::download_open::describe(&*c);
            Ok(if l.is_empty() { "No saved downloads.".into() } else { l.join("\n") })
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        let schema = json!({"type": "object", "properties": {"name": {"type": "string", "description": "The file name as listed (no folder)."}}, "required": ["name"]});
        v.push(
            Entry::change("Browser", "browser_download_open", "Open a saved download in the tile that suits it (EDIT, DATA or MEDIA); the screen the person is on is kept aside. The person approves a card first.", schema, |a| format!("Open the download {}", a["name"].as_str().unwrap_or("")), move |a| {
                let name = arg_str(a, "name")?;
                let (tile, path) = super::download_open::resolve(&*c, &name)?;
                super::download_open::queue_chat(&c.user(), tile, &path);
                e1.audit("browser.download.open", tile);
                Ok(format!("Opening {name} in {}.", tile.to_ascii_uppercase()))
            })
            .with_refusal(|a| {
                let n = a["name"].as_str().unwrap_or("");
                (n.is_empty() || n.contains(['/', '\\']) || n.starts_with('.')).then(|| "name is a plain file name from the Downloads list".to_string())
            }),
        );
    }
    {
        let c = cx.shared();
        v.push(Entry::read("Browser", "browser_reports_list", "List the site reports the person saved with the Report site button (or the ! key) in the developer tools: time, site, status, engine and the number of notes. Reports stay on this computer.", no_args(), move |_| {
            let path = super::browser_report::file_for(c.data_dir(), &c.user());
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let rows: Vec<String> = text
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .map(|v| format!("{} {} status {} ({}), {} notes", v["at"], v["host"].as_str().unwrap_or("?"), v["status"], v["engine"].as_str().unwrap_or("?"), v["notes"].as_array().map_or(0, Vec::len)))
                .collect();
            Ok(if rows.is_empty() { "No reports saved.".into() } else { rows.join("\n") })
        }));
    }
    {
        let (c, e1) = (cx.shared(), env.clone());
        v.push(Entry::change("Browser", "browser_reports_clear", "Delete the person's saved site reports. The person approves a card first.", no_args(), |_| "Delete the saved site reports".to_string(), move |_| {
            let path = super::browser_report::file_for(c.data_dir(), &c.user());
            let had = path.is_file();
            if had {
                std::fs::remove_file(&path).map_err(|e| format!("could not delete the reports: {e}"))?;
            }
            e1.audit("browser.report.clear", "");
            Ok(if had { "The saved site reports were deleted.".into() } else { "There were no reports.".into() })
        }));
    }
    v
}

/// Where this person's pictures are (the browser tile uses the same folder).
fn thumbs_dir(cx: &dyn ChatPort) -> std::path::PathBuf {
    cx.data_dir().join(format!("browser-thumbs-{}", super::browser_config::who(cx)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_shared::agent::{Risk};
    use iyakki_shared::agent::{Tools};
    use iyakki_shared::settingstools::SettingsTools;
    use std::sync::{Arc, Mutex};

    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _a: &serde_json::Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _a: &serde_json::Value) -> String {
            name.to_string()
        }
    }

    fn rig(tag: &str) -> (AppCx, SettingsTools, Arc<Mutex<Vec<String>>>) {
        let cx = cx_with_admin_store(tag);
        cx.services.on_login("root", "rootpass12", true);
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&cx.data_dir().join("settings.json")).unwrap()));
        let env = Env { dir: cx.data_dir().to_path_buf(), user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        let t = SettingsTools::new(Arc::new(NoTools), true, entries(&CxChat(&cx), &env));
        (cx, t, audits)
    }

    #[test]
    fn reads_are_free_changes_ask_and_the_tools_do_what_the_browser_does() {
        let (cx, t, audits) = rig("chatbrowserdata");
        let defs = t.defs();
        for n in ["browser_bookmarks", "browser_history_search", "browser_zoom_status", "browser_profile_status"] {
            assert_eq!(defs.iter().find(|d| d.spec.name == n).unwrap().risk, Risk::Safe, "{n}");
        }
        for n in ["browser_bookmark_folder", "browser_zoom_set", "browser_profile_clear"] {
            assert_eq!(defs.iter().find(|d| d.spec.name == n).unwrap().risk, Risk::Ask, "{n}");
        }
        let mut d = bd::Data::default();
        d.toggle_bookmark("https://a.org/x", "Alpha");
        d.visit("https://b.org/page", "Bravo news", 5);
        bd::save(&cx, &d);
        // folders: made, filled by site, listed, renamed, deleted
        assert!(t.call("browser_bookmark_folder", &json!({"action": "new", "name": "Work"})).unwrap().contains("Work"));
        assert!(t.call("browser_bookmark_folder", &json!({"action": "move", "name": "Work", "site": "a.org"})).unwrap().contains("Moved 1"));
        assert_eq!(t.call("browser_bookmarks", &json!({})).unwrap(), "(top level): 0\nWork: 1");
        assert!(t.call("browser_bookmarks", &json!({"folder": "Work"})).unwrap().contains("Alpha (a.org)"));
        assert!(t.call("browser_bookmark_folder", &json!({"action": "move", "name": "Work", "site": "none.org"})).is_err());
        t.call("browser_bookmark_folder", &json!({"action": "rename", "name": "Work", "to": "Jobs"})).unwrap();
        assert!(t.call("browser_bookmark_folder", &json!({"action": "delete", "name": "Jobs"})).unwrap().contains("1 bookmarks"));
        assert_eq!(bd::load(&cx).bookmarks.len(), 1, "deleting a folder keeps the bookmark");
        // history: titles and sites, never the address path
        let r = t.call("browser_history_search", &json!({"query": "bravo"})).unwrap();
        assert!(r.contains("Bravo news (b.org)") && !r.contains("/page"), "{r}");
        assert!(t.call("browser_history_search", &json!({"query": "zzz"})).unwrap().contains("Nothing"));
        // zoom
        assert_eq!(t.call("browser_zoom_status", &json!({})).unwrap(), "Every site is at 100%.");
        t.call("browser_zoom_set", &json!({"site": "A.org", "percent": 150, "text_only": true})).unwrap();
        assert_eq!(t.call("browser_zoom_status", &json!({})).unwrap(), "a.org: 150% (text only)");
        assert_eq!(bd::load(&cx).zoom_for("a.org"), 1.5);
        t.call("browser_zoom_set", &json!({"site": "a.org", "percent": 100})).unwrap();
        assert!(bd::load(&cx).zoom.is_empty());
        assert!(t.refusal("browser_zoom_set", &json!({"site": "a b", "percent": 150})).is_some() && t.refusal("browser_zoom_set", &json!({"site": "a.org", "percent": 900})).is_some());
        // the saved name and address: field names only
        assert_eq!(t.call("browser_profile_status", &json!({})).unwrap(), "Nothing is kept.");
        let mut p = bf::Profile::default();
        p.set(bf::Field::Email, "ada@example.org").unwrap();
        p.set(bf::Field::Street, "12 Analytical Way").unwrap();
        bf::save(&cx, &p).unwrap();
        let r = t.call("browser_profile_status", &json!({})).unwrap();
        assert_eq!(r, "Kept: email, street.");
        assert!(!r.contains("ada") && !r.contains("Analytical"));
        t.call("browser_profile_clear", &json!({})).unwrap();
        assert!(bf::load(&cx).unwrap().is_empty());
        // page pictures
        let th = super::super::browser_thumbs::Thumbs::new(&thumbs_dir(&CxChat(&cx)));
        th.save("a.org", &tiny_skia::Pixmap::new(super::super::browser_thumbs::W, super::super::browser_thumbs::H).unwrap()).unwrap();
        assert_eq!(t.call("browser_thumbnails_status", &json!({})).unwrap(), "Pictures of often-visited pages are on; 1 kept.");
        t.call("browser_thumbnails_set", &json!({"mode": "off"})).unwrap();
        assert_eq!(t.call("browser_thumbnails_status", &json!({})).unwrap(), "Pictures of often-visited pages are off; 0 kept.");
        assert!(bd::load(&cx).thumbs_off);
        assert!(t.call("browser_thumbnails_set", &json!({"mode": "maybe"})).is_err());
        t.call("browser_thumbnails_set", &json!({"mode": "on"})).unwrap();
        assert!(!bd::load(&cx).thumbs_off);
        let log = audits.lock().unwrap().join("|");
        assert!(log.contains("settings.browser") && !log.contains("ada@example") && !log.contains("Analytical"), "{log}");
    }

    #[test]
    fn the_media_policy_is_read_free_and_changed_only_after_a_card_with_values_checked() {
        let (cx, t, audits) = rig("chatbrowsermediapol");
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_media_policy_status").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_media_policy_set").unwrap().risk, Risk::Ask);
        assert_eq!(t.call("browser_media_policy_status", &json!({})).unwrap(), "Video quality: Automatic (no limit). Add-on decoders: allowed.");
        assert!(t.call("browser_media_policy_set", &json!({"max_height": 999})).is_err(), "only the listed heights");
        assert!(t.call("browser_media_policy_set", &json!({})).is_err(), "something must be said");
        let r = t.call("browser_media_policy_set", &json!({"max_height": 720, "codecs": false})).unwrap();
        assert!(r.contains("up to 720 pixels tall") && r.contains("off"), "{r}");
        let b = super::super::browser_config::load(&cx);
        assert_eq!((b.video_max_height, b.codecs), (720, false));
        assert!(audits.lock().unwrap().iter().any(|l| l.contains("Video quality")), "audited");
    }

    #[test]
    fn hardware_video_decoding_is_read_free_and_changed_only_after_a_card_by_an_administrator_and_audited() {
        let (cx, t, _audits) = rig("chatbrowserhwdec");
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_hw_decode_status").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_hw_decode_set").unwrap().risk, Risk::Ask);
        assert!(t.call("browser_hw_decode_status", &json!({})).unwrap().starts_with("Hardware video decoding: off."));
        assert!(t.call("browser_hw_decode_set", &json!({})).is_err(), "say on");
        let r = t.call("browser_hw_decode_set", &json!({"on": true})).unwrap();
        assert!(r.contains("Hardware video decoding: on."), "{r}");
        assert!(super::super::browser_tuning::load(&cx));
        assert!(t.call("browser_hw_decode_status", &json!({})).unwrap().starts_with("Hardware video decoding: on."));
        assert!(cx.audit.recent(40).join("\n").contains("Hardware video decoding: on."), "audited");
        t.call("browser_hw_decode_set", &json!({"on": false})).unwrap();
        assert!(!super::super::browser_tuning::load(&cx));
    }

    #[test]
    fn the_av_addon_status_is_free_and_says_where_to_install() {
        let (_cx, t, _audits) = rig("chatbrowseraddon");
        assert_eq!(t.defs().iter().find(|d| d.spec.name == "browser_av_addon_status").unwrap().risk, Risk::Safe);
        let r = t.call("browser_av_addon_status", &json!({})).unwrap();
        assert!(r.starts_with("Audio and video add-on"), "{r}");
    }

    #[test]
    fn remembered_media_sites_are_listed_free_and_forgotten_only_when_asked_and_never_allowed_by_chat() {
        use iyakki_webview::proto::media_kind::CAMERA;
        let (cx, t, audits) = rig("chatbrowsermedia");
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_media_sites").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_media_site_forget").unwrap().risk, Risk::Ask);
        assert!(t.call("browser_media_sites", &json!({})).unwrap().contains("every request asks"));
        let mut m = super::super::browser_media::MediaPerms::default();
        m.apply("https://meet.example/r", CAMERA, super::super::browser_media::Answer::Always);
        super::super::browser_media::save(&cx, &m);
        assert_eq!(t.call("browser_media_sites", &json!({})).unwrap(), "meet.example: camera allowed");
        let mut m2 = super::super::browser_media::load(&cx);
        m2.set_site_device("meet.example", CAMERA, "USB Cam: x");
        super::super::browser_media::save(&cx, &m2);
        assert_eq!(t.call("browser_media_sites", &json!({})).unwrap(), "meet.example: camera allowed (camera: USB Cam: x)", "the camera chosen for the site is named");
        assert!(t.call("browser_media_site_forget", &json!({"site": "other.example"})).is_err());
        assert!(t.refusal("browser_media_site_forget", &json!({"site": "a b"})).is_some() || t.call("browser_media_site_forget", &json!({"site": "a b"})).is_err());
        t.call("browser_media_site_forget", &json!({"site": "Meet.example"})).unwrap();
        assert!(super::super::browser_media::load(&cx).list().is_empty());
        assert!(audits.lock().unwrap().join("|").contains("meet.example forgotten"));
    }

    #[test]
    fn the_protection_choices_are_read_free_changed_after_approval_and_bad_sites_are_refused() {
        let (cx, t, audits) = rig("chatprotectchoices");
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_protection_choices").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "browser_protection_choices_set").unwrap().risk, Risk::Ask);
        let r = t.call("browser_protection_choices", &json!({})).unwrap();
        assert!(r.contains("Timed checkpoint: every 10 min") && r.contains("Seal when the screen locks: no"), "{r}");
        let a = json!({"checkpoint_minutes": 5, "lock_seal": true, "never_persist": ["Shop.Example.com", "https://bank.example.org/"], "clear_cookies_on_exit": true});
        assert!(t.describe("browser_protection_choices_set", &a).contains("lock_seal"));
        let said = t.call("browser_protection_choices_set", &a).unwrap();
        assert!(said.contains("every 5 min") && said.contains("bank.example.org, shop.example.com") && said.contains("is off"), "{said}");
        let p = iyakki_shared::profilepolicy::Policy::load(cx.data_dir(), "root");
        assert!(p.lock_seal && p.clear_cookies_on_exit && p.checkpoint_minutes == 5);
        // only the fields given change
        t.call("browser_protection_choices_set", &json!({"lock_seal": false})).unwrap();
        let p = iyakki_shared::profilepolicy::Policy::load(cx.data_dir(), "root");
        assert!(!p.lock_seal && p.checkpoint_minutes == 5 && p.never_persist.len() == 2);
        for bad in [json!({"never_persist": ["not a site"]}), json!({"checkpoint_minutes": 5000}), json!({"lock_seal": "yes"}), json!({"clear_sites_on_exit": "a.com"})] {
            assert!(t.call("browser_protection_choices_set", &bad).is_err(), "{bad}");
        }
        assert!(audits.lock().unwrap().iter().any(|a| a.contains("settings.browser.protection")));
    }

    #[test]
    fn the_sealed_browser_data_is_exported_and_imported_only_after_approval_with_the_vault_key_and_never_open() {
        use iyakki_shared::profilestore::{Status};
        use iyakki_agent::profilestore::{ProfileKey, ProfileStore};
        let (cx, t, audits) = rig("chatdatamove");
        let defs = t.defs();
        for n in ["browser_data_export", "browser_data_import"] {
            assert_eq!(defs.iter().find(|d| d.spec.name == n).unwrap().risk, Risk::Ask, "{n}");
        }
        let dir = cx.data_dir().to_path_buf();
        let out = dir.join("moved.iykbp");
        let a = json!({"path": out.display().to_string()});
        // nothing protected yet
        assert!(t.call("browser_data_export", &a).unwrap_err().contains("not protected"));
        // a protected, sealed profile whose key is in the person's vault
        let st = ProfileStore::new(&dir, "root");
        std::fs::create_dir_all(st.profile_path().join("data")).unwrap();
        std::fs::create_dir_all(st.profile_path().join("config")).unwrap();
        std::fs::write(st.profile_path().join("data/cookies.sqlite"), b"cookie rows").unwrap();
        let key = ProfileKey::generate().unwrap();
        st.protect(&key).unwrap();
        cx.vault().user_set("root", iyakki_shared::profilestore::VAULT_SECRET, &key.to_hex()).unwrap();
        for bad in [json!({"path": "relative.iykbp"}), json!({"path": "/tmp/../etc/x"})] {
            assert!(t.call("browser_data_export", &bad).is_err(), "{bad}");
        }
        let said = t.call("browser_data_export", &a).unwrap();
        assert!(said.contains("not in the file") && out.exists(), "{said}");
        assert!(audits.lock().unwrap().iter().any(|l| l.starts_with("settings.browser.data.export")));
        assert!(t.call("browser_data_export", &a).is_err(), "never over a file");
        // take it in again: the same key opens it; a wrong or missing key and an open profile refuse
        assert!(t.call("browser_data_import", &a).unwrap().contains("taken in"));
        assert_eq!(st.status(), Status::Sealed);
        let other = ProfileKey::generate().unwrap();
        cx.vault().user_set("root", iyakki_shared::profilestore::VAULT_SECRET, &other.to_hex()).unwrap();
        assert!(t.call("browser_data_import", &a).unwrap_err().contains("wrong key"));
        cx.vault().user_set("root", iyakki_shared::profilestore::VAULT_SECRET, &key.to_hex()).unwrap();
        st.open(&key).unwrap();
        assert!(t.call("browser_data_import", &a).unwrap_err().contains("open"));
        assert!(t.call("browser_data_export", &json!({"path": dir.join("again.iykbp").display().to_string()})).unwrap_err().contains("open"));
    }

    #[test]
    fn the_sealed_browser_data_goes_to_a_signed_in_tenant_and_back_only_after_approval_with_the_vault_key() {
        use iyakki_agent::profilestore::{ProfileKey, ProfileStore};
        use iyakki_shared::profilestore::Status;
        struct Slot(std::sync::Mutex<Option<Vec<u8>>>);
        impl iyakki_agent_api::ProfileCarrier for Slot {
            fn put(&self, b: &[u8]) -> Result<(), String> {
                *self.0.lock().unwrap() = Some(b.to_vec());
                Ok(())
            }
            fn get(&self) -> Result<Option<Vec<u8>>, String> {
                Ok(self.0.lock().unwrap().clone())
            }
        }
        let (cx, t, audits) = rig("chatdatatenant");
        let defs = t.defs();
        for n in ["browser_data_to_tenant", "browser_data_from_tenant"] {
            assert_eq!(defs.iter().find(|d| d.spec.name == n).unwrap().risk, Risk::Ask, "{n}");
        }
        let dir = cx.data_dir().to_path_buf();
        let a = json!({"tenant": "chat-move-tenant"});
        // a tenant that is not signed in is named in words
        assert!(t.call("browser_data_to_tenant", &a).unwrap_err().contains("not signed in"));
        let st = ProfileStore::new(&dir, "root");
        std::fs::create_dir_all(st.profile_path().join("data")).unwrap();
        std::fs::write(st.profile_path().join("data/cookies.sqlite"), b"cookie rows").unwrap();
        let key = ProfileKey::generate().unwrap();
        st.protect(&key).unwrap();
        cx.vault().user_set("root", iyakki_shared::profilestore::VAULT_SECRET, &key.to_hex()).unwrap();
        let slot = std::sync::Arc::new(Slot(Default::default()));
        cx.agent.profile_store().set_tenant_carrier("chat-move-tenant", Some(slot.clone()));
        assert!(cx.agent.profile_store().tenant_carriers().contains(&"chat-move-tenant".to_string()));
        let said = t.call("browser_data_to_tenant", &a).unwrap();
        assert!(said.contains("cannot") || said.contains("opens only with the browser key"), "{said}");
        assert!(slot.0.lock().unwrap().is_some());
        assert!(audits.lock().unwrap().iter().any(|l| l.starts_with("settings.browser.data.to_tenant")));
        // back: the same key takes it in; another key refuses
        assert!(t.call("browser_data_from_tenant", &a).unwrap().contains("taken in"));
        assert_eq!(st.status(), Status::Sealed);
        let other = ProfileKey::generate().unwrap();
        cx.vault().user_set("root", iyakki_shared::profilestore::VAULT_SECRET, &other.to_hex()).unwrap();
        assert!(t.call("browser_data_from_tenant", &a).unwrap_err().contains("wrong key"));
        cx.agent.profile_store().set_tenant_carrier("chat-move-tenant", None);
        assert!(t.call("browser_data_from_tenant", &a).unwrap_err().contains("not signed in"));
    }
}

#[cfg(test)]
#[path = "chat_browser_downloads_tests.rs"]
mod downloads_tests;
