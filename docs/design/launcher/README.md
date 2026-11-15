# Home screen — locked design, November 2026

Open `index.html` in a browser for the full render: the desktop, the widget board, edit mode and all five app
pages. Every screen in it is the live `launcher.html`, not a picture, so the paging and the motion work.
`launcher.html?p=0..5`, `?edit=1`, `?desktop=1` address the screens directly.

These are **design references, not product code.** They are HTML because that is the fastest way to decide a
direction; IYAKKI draws its own pixels with `tiny-skia` and has no DOM. What ships is Rust.

## What is locked

| | |
|---|---|
| Structure | Six pages side by side: *The machine*, then the five groups. Two-finger swipe, `←` `→`, `1`–`6`, edge chevrons, page dots |
| Surface | A sharp desktop wallpaper with **one frosted island** on top: 40 px radius, heavy blur, white rim, gloss band down the top, ambient shadow |
| Icons | 108 px squircles, per-app hue on a gradient, top gloss, rim light, white glyph. State never changes the icon colour — it lives on the corner badge and the chip under the label |
| Motion | The island pops from 0.975 on a page change; icons stagger in 0.72 → 1.05 → 1.0, 42 ms apart. `prefers-reduced-motion` (and so `IYAKKI_REDUCED_MOTION`) removes both |
| Page 1 | A **customisable widget board**. Edit mode gives every widget a remove badge, a drag grip and a size selector, plus a slot to add more |
| Status | **Attention first**: the apps that need a person, named with the actual problem, then one line for the 30 that are fine |
| Colour | Light throughout. No dark surfaces |

## What landed in code with this commit

Only the palette: `crates/iyakki-uikit/src/theme.rs`, the `LIGHT` constant. That is deliberate — `Gfx` is the one
place a colour becomes pixels and it consults `themed()` first, so re-pointing `LIGHT` re-skins **all 37 apps** at
once rather than one screen. `THEME=light ./run.sh screens` renders every app in it.

## Not built yet

1. **Wallpaper.** IYAKKI has none. `chat_toolscore_reqs.rs:105` registers "change my wallpaper" as *no such tool*.
   Needs a setting, a shipped default and a picker. `wall.svg` here is a stand-in for the comps.
2. **Blur.** `tiny-skia` has no blur primitive. The island, the dock and the menu bar all want one. The wallpaper
   is static, so the plan is to blur it once into a cached pixmap and sample that at an offset while the island
   slides — not a per-frame blur.
3. **Paged launcher, island, widget board, icon set.** The structural work in `launcher.rs` and `iyakki-uikit`.
4. **`status()` on the `App` trait, 37 implementations.** Until that exists every figure on page 1 and every
   subtitle under an icon is fiction. This is the long pole, and the design is worth little without it.
