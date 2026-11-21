//! What the client page tells the person about the device key is true in each place it runs: in a browser tab the browser keeps it; in the
//! window program (`iyakki-connect window`, which serves its own `keys.js`) the program keeps it. Source-level checks; the page itself ran in
//! headless Chrome in `window_browser.rs` (ignored by default).

const PAGE_JS: &str = include_str!("../src/clientpage/page.js");
const WINDOW_KEYS: &str = include_str!("../../iyakki-connect/web/window-keys.js");
const BROWSER_KEYS: &str = include_str!("../../iyakki-client-engine/js/keys.js");

#[test]
fn the_enrol_sentence_about_the_key_is_chosen_by_who_holds_the_key() {
    let at = PAGE_JS.find("$(\"keyinfo\").textContent =").expect("the enrol form sets the key sentence");
    let line = PAGE_JS[at..].lines().next().unwrap();
    assert!(line.contains("keys.heldByProgram ? keys.describeKey(null)"), "the program's own words come first: {line}");
    assert!(line.contains("the browser will keep it"), "the browser sentence stays for a browser tab: {line}");
    // the window program's keys.js says the program holds it and marks itself; the browser's keys.js does not
    assert!(WINDOW_KEYS.contains("export const heldByProgram = true;"));
    assert!(WINDOW_KEYS.contains("held by the iyakki-connect program on this computer, not by this page"));
    assert!(!BROWSER_KEYS.contains("heldByProgram"), "a browser tab must not claim the program holds the key");
}

#[test]
fn the_page_opens_an_apps_display_list_in_a_window_or_the_phone_form_only_when_the_machine_agreed() {
    const INDEX: &str = include_str!("../src/clientpage/index.html");
    const CSS: &str = include_str!("../src/clientpage/page.css");
    assert!(PAGE_JS.contains("import(\"/connect/uidisplay.js\")"), "the page loads the one shared module");
    assert!(PAGE_JS.contains("mountDisplayList({ engine, canvas"), "the page mounts it with the engine and its own canvas");
    assert!(INDEX.contains("id=\"app\"") && INDEX.contains("id=\"app-aria\""), "a canvas and an accessibility host for the app");
    // the additive message pair: the page reaches it through the shared link, which makes the engine commands app_open, app_input and app_close
    assert!(PAGE_JS.contains("createAppLink({ engine, pump") && PAGE_JS.contains("appLink.openApp(id, plan.w, plan.h)"), "the Apps menu opens an app through the link");
    assert!(PAGE_JS.contains("case \"app\": if (files && files.onMessage(o.body)) break; if (appLink) appLink.onRecord(o)"), "the engine's app records reach the file transfer first and then the link");
    // file transfer (F-CLIENT-01): its own module, loaded like the media half, with the page's controls; nothing is inline
    assert!(PAGE_JS.contains("import(\"/connect/files.js\")") && PAGE_JS.contains("files.onBinary(m.data)") && PAGE_JS.contains("if (files) files.reset();"));
    assert!(iyakki_remote::clientpage::PATHS.contains(&"/connect/files.js") && iyakki_remote::clientpage::serve("/connect/files.js").is_some());
    assert!(INDEX.contains("id=\"files-row\"") && INDEX.contains("id=\"files-pick\" type=\"file\""), "the file controls exist and stay hidden until the machine agreed");
    for cmd in ["app_open", "app_input", "app_close"] {
        assert!(iyakki_client_engine::UIDISPLAY_JS.contains(cmd), "the shared module makes {cmd}");
    }
    // the button exists only when the machine agreed to the capability; an older machine shows no Apps button and the page is as before
    assert!(PAGE_JS.contains("agreed.includes(\"display.list\")") && PAGE_JS.contains("$(\"apps\").hidden = !on"));
    assert!(INDEX.contains("id=\"apps\"") && INDEX.contains("id=\"apps\" type=\"button\" aria-label=\"Apps of the IYAKKI machine\" hidden"), "hidden until agreed");
    // the window and the phone form, a title bar with Close, the resize and the machine's screen behind
    assert!(PAGE_JS.contains("uiDisplay.panePlan(") && PAGE_JS.contains("appView.resize(plan.w, plan.h)"));
    assert!(INDEX.contains("id=\"app-title\"") && INDEX.contains("id=\"app-close\""));
    assert!(CSS.contains("#stage[data-appform=\"window\"]") && CSS.contains("#stage[data-appform=\"phone\"]"), "both forms have a style");
    // the content security policy still holds: no inline script or style was added for the app view
    assert!(!INDEX.contains("<script>") && INDEX.matches("style=\"").count() <= 1, "no inline script, and no inline style beyond the old noscript line");
    assert!(iyakki_remote::clientpage::PATHS.contains(&"/connect/uidisplay.js"));
    assert!(iyakki_remote::clientpage::serve("/connect/uidisplay.js").is_some(), "the module is served");
    let served = iyakki_remote::clientpage::serve("/connect/uidisplay.js").unwrap();
    assert!(String::from_utf8_lossy(&served.body).contains("export function panePlan"), "the served module has the window and phone plan");
}
