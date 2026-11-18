//! M66: proof for rows whose Notes named a missing assertion: the run footer's token rate (F-ASSIST-46).

use super::*;

#[test]
fn the_run_footer_shows_the_token_rate_only_when_the_run_was_long_and_big_enough_to_mean_something() {
    let mut app = ChatApp::new();
    assert_eq!(app.usage_text(), "", "no run yet: no footer");
    // (tokens, cents, files, seconds)
    app.usage = Some((1200, 0.0, 0, 10));
    assert_eq!(app.usage_text(), "~1.2k tokens · ~120 tok/s");
    app.usage = Some((450, 0.0, 0, 9));
    assert_eq!(app.usage_text(), "~450 tokens · ~50 tok/s");
    // under three seconds, or under thirty tokens, the rate is left out (a rate from a blink is noise)
    app.usage = Some((450, 0.0, 0, 2));
    assert_eq!(app.usage_text(), "~450 tokens");
    app.usage = Some((29, 0.0, 0, 20));
    assert_eq!(app.usage_text(), "~29 tokens");
    app.usage = Some((30, 0.0, 0, 3));
    assert_eq!(app.usage_text(), "~30 tokens · ~10 tok/s");
    // never zero: a slow run still says 1
    app.usage = Some((40, 0.0, 0, 3600));
    assert_eq!(app.usage_text(), "~40 tokens · ~1 tok/s");
    // the cost and the files follow the rate, as the footer shows them
    app.usage = Some((2000, 12.0, 1, 4));
    assert_eq!(app.usage_text(), "~2.0k tokens · ~500 tok/s · $0.12 · 1 file");
    app.usage = Some((100, 0.0, 3, 5));
    assert!(app.usage_text().ends_with(" · 3 files"), "{}", app.usage_text());
}
