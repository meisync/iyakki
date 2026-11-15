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
