# Themes, skins and fonts

What the person chooses (a Look, a skin, a font, a text size) must change your app without a line of your code. That works only if you draw with what the SDK hands you. This page is the short form of `docs/development/UI-SDK.md` (section "Themes, skins and fonts") and `LOOK-DESIGN.md`.

## Colours, tokens, type: ask, never write

| You need | Ask | Never |
|---|---|---|
| a colour | `cx.colors()` (`th.text`, `th.muted`, `th.accent`, `th.card`, `th.border`, ...) | `rgb(..)`, `[12, 34, 56, 255]` |
| a text size | `m.size(TypeRole::Body)` (Caption, Small, Body, Label, Large, Title, Heading, Display) | `14.0` |
| a gap or padding | `m.gap(Space::Md)` (Xxs, Xs, Sm, Md, Lg, Xl), `m.gutter()` | `8.0`, `12.0` |
| a corner radius | `m.radius(Radius::Md)` (Sm, Md, Lg), `m.tokens.radius_for(r, height)` | `6.0` |
| a border width | `m.line(Line::Normal)` (Hair, Normal, Thick) | `1.0`, `2.0` |
| a card or panel | `widgets::surface(c, th, &m, rect, fill, Radius::Md, raised)` | your own rounded rectangle and shadow |
| a target size | `m.min_target()` (44 on touch, 24 with a pointer) | `40.0` |
| a font | a role: `FontRole::Body / Ui / Mono / Display`, `m.text_style(role, TypeRole::Body)` | a font file or name |
| an accent of your own | `cx.accent(wanted)` (fitted to contrast; says if it was `adjusted`) | drawing your accent unchecked |

Numbers under 3 (halves, doubles, ratios such as `size * 1.5`) are fine; so is a `const` line that names a proportion or a count. A divisor such as "three tabs" trips the lint (`/ 3.0`): write `const TABS: f32 = 3.0;` and divide by `TABS`.

## The style-literal lint

`iyakki_ui_sdk::lint::style_literals(source)` returns the lines that hold a decimal number of 3.0 or more outside comments, strings, `const` lines, `em(` arguments and test modules. The generated app has the test `no_styling_numbers_in_the_drawing_code`. When it fails the message lists `line N: <code>` and the fix: replace the number by the step it stands for (the table above). Before and after:

```rust
// before: fails the lint, ignores the skin and the person's text size
c.text(g, y, 14.0, th.text, "Hello", TextStyle::Regular);
let gap = 8.0;
// after
c.text(g, y, m.size(TypeRole::Body), th.text, "Hello", TextStyle::Regular);
let gap = m.gap(Space::Md);
```

## Size classes

`cx.size_class()` is `Compact` (under 600 units wide: a phone or a narrow pane, one column), `Medium` (under 900: a side panel may appear) or `Expanded`. The pane can change size at any moment (`UiEvent::Resized`); `view` asks again each frame. Test at 360x740, 800x480 and 1280x720.

## Text at 3x and contrast

The person's text size runs from 0.5x to 3x; `m.size` and `m.font` apply it. At 3x your layout must still fit: nothing outside the pane, every target at least the minimum, text shortened with an ellipsis (`canvas::fit`) rather than overflowing. Test it with `host.text_scale = 3.0`. Contrast: text on a surface is at least 4.5 to 1 and an accent against its card 3 to 1; use the theme's colours and `cx.accent` and you inherit that. The SDK's own `type_tests` run the widgets at Dark, Light, High contrast and the Paper skin, three size classes, text 1x, 2x and 3x, both font families.

## Skins

A skin is a Look file (`crates/iyakki-uikit/skins/*.look`): the colours plus shape, spacing, glass and motion. The person picks one in Settings, Appearance (or by chat: `display_look_set`); the shell calls `Host::set_look` and the next `view` draws in it, with no restart. Shipped: Paper, Glass night, Blueprint, Calm. To see your app in each, loop over `skins` in a test and call `set_look`.

## What is not available

No CJK font and no dyslexia-friendly font is bundled, so neither is offered. A skin file cannot carry type settings. The pixel canvas draws the person's one family; a single run cannot switch family there.
