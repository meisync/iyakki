//! The terminal installer: the [`Wizard`] drawn on a terminal, arrow keys to move. Used by `iyakki-usb --plan` on a computer
//! and by the boot-time install mode on the console.

use std::io::Write;
use std::time::{Duration, Instant};

use crate::planner::Plan;
use crate::screen::Zoom;
use crate::term::{width, Raw};
use crate::wizard::{parse_wkeys, Next, Page, WKey, Wizard};
use crate::writer::Progress;

/// What writes a confirmed plan. It receives a progress callback; `Ok` is the final message.
pub type Writer<'a> = &'a mut dyn FnMut(&Plan, &mut dyn FnMut(Progress)) -> Result<String, String>;

/// Run the pages until the person quits or the install ends. Returns whether anything was written successfully.
pub fn run(wiz: &mut Wizard, writer: Writer) -> Result<bool, String> {
    let raw = Raw::enter().ok_or("the installer needs a terminal")?;
    // bigger text on a high-resolution console (restored when this function returns); + and - change it
    let mut zoom = Zoom::start();
    let mut wrote = false;
    loop {
        print!("{}", wiz.render(width()));
        let _ = std::io::stdout().flush();
        for k in parse_wkeys(&raw.read_bytes()) {
            if let WKey::Char(c) = k {
                if zoom.key(c) {
                    print!("{}", wiz.render(width()));
                    let _ = std::io::stdout().flush();
                    continue;
                }
            }
            match wiz.handle(k) {
                Next::Nothing => {}
                Next::Quit => {
                    drop(zoom);
                    drop(raw);
                    println!();
                    return Ok(wrote);
                }
                Next::Write => {
                    let plan = wiz.plan.clone().ok_or("internal error: no plan to write")?;
                    let mut last = Instant::now() - Duration::from_secs(1);
                    let res = {
                        let mut cb = |p: Progress| {
                            if last.elapsed() < Duration::from_millis(200) && p.done < p.total {
                                return;
                            }
                            last = Instant::now();
                            wiz.progress = (format!("{}  {}", p.phase.label(), crate::ui::human_duration(p.elapsed)), p.done, p.total);
                            print!("{}", wiz.render(width()));
                            let _ = std::io::stdout().flush();
                        };
                        writer(&plan, &mut cb)
                    };
                    wrote = res.is_ok();
                    wiz.finished(res);
                    break;
                }
            }
            if wiz.page == Page::Progress {
                break;
            }
        }
    }
}
