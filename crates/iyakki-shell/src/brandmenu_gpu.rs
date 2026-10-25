//! The GPU page of the IYAKKI menu (child module of `brandmenu`): the graphics devices found (PCI ids to names from the in-house table), the
//! state of their drivers, the pods that use them, the Vulkan and NVIDIA install state and the test prompt (tokens per second).
//! The facts come from `iyakki_agent::gpu_steps` (files at once; the kernel log and the cluster on a thread). It starts nothing itself:
//! the buttons return outcomes (`GpuTest`, `Opened(Page::Gpu)` to read again, Store -> Setup).

use super::*;
use iyakki_agent::gpu;

/// Run the test prompt (a detached job; its log shows the answer and the tokens per second).
pub const GPU_TEST: usize = BASE + 80;
/// Read the page's facts again.
pub const GPU_REFRESH: usize = BASE + 81;
/// Open Store -> Setup (the GPU cards are in Hardware).
pub const GPU_INSTALL: usize = BASE + 82;
/// The "Open its page" button of the GPU cards of Store -> Setup.
pub const GPU_PAGE: usize = BASE + 83;

impl BrandMenu {
    /// The page's facts (the shell fills them when the page opens).
    pub fn set_gpu(&mut self, r: gpu::Report) {
        self.gpu = Some(r);
    }

    /// The next selected row in a direction.
    pub(super) fn step_sel(&self, dir: i32, n: usize) -> usize {
        ((self.sel as i32 + dir).rem_euclid(n.max(1) as i32)) as usize
    }

    pub(super) fn gpu_hit(&mut self, ev: &Event) -> Option<Outcome> {
        match ev {
            Event::Tab(GPU_TEST) => Some(Outcome::GpuTest),
            Event::Tab(GPU_REFRESH) => Some(Outcome::Opened(Page::Gpu)),
            Event::Tab(GPU_INSTALL) => Some(Outcome::Jump("store", "setup".into())),
            Event::Tab(GPU_PAGE) => Some(self.open_page(Page::Gpu)),
            _ => None,
        }
    }

    pub(super) fn draw_gpu(&mut self, g: &mut Gfx, body: R, s: f32) {
        // the slow part (kernel log, pods) arrives on its own
        if let (Some(b), Some(r)) = (iyakki_agent::gpu_steps::take_background(), self.gpu.as_mut()) {
            iyakki_agent::gpu_steps::merge_background(r, &b, std::path::Path::new("/"));
        }
        let lines = self.gpu.as_ref().map(gpu::report_lines).unwrap_or_else(|| vec![(0, "looking...".to_string())]);
        let bottom = body.y + body.h - 44.0 * s;
        let lh = 17.0 * s;
        let mut y = body.y;
        'all: for (level, text) in &lines {
            let col = match level {
                1 => OK,
                2 => AMBER,
                3 => ERR,
                _ => MUTED,
            };
            let indent = text.len() - text.trim_start().len();
            for l in g.wrap(12.5 * s, text.trim_start(), Face::Sans, body.w - (indent as f32 * 4.0 + 8.0) * s) {
                if y + lh > bottom {
                    g.text(body.x, y, 12.0 * s, DIM, "\u{2026} (more in the kernel log: Settings \u{2192} System)", Face::Sans);
                    break 'all;
                }
                g.text(body.x + (indent as f32 * 4.0) * s, y, 12.5 * s, col, &l, Face::Sans);
                y += lh;
            }
            y += 3.0 * s;
        }
        let by = bottom + 6.0 * s;
        let mut x = body.x;
        x += g.button(x, by, 14.0 * s, "Run test prompt", ACCENT, Event::Tab(GPU_TEST)) + 10.0 * s;
        x += g.button(x, by, 14.0 * s, "Refresh", MUTED, Event::Tab(GPU_REFRESH)) + 10.0 * s;
        g.button(x, by, 14.0 * s, "Store \u{2192} Setup", MUTED, Event::Tab(GPU_INSTALL));
    }
}

#[cfg(test)]
#[path = "brandmenu_gpu_tests.rs"]
mod tests;
