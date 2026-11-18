# Pane skeletons

All of these exist as compiling files in `docs/sdk/recipes/`; copy the file, then change names.

## One pane app (what `new-app` makes)

State struct, `const A_X: u32` action numbers, `impl UiApp` with `event` (match on `UiEvent::Action(A_X)`, `Key`, `Text`, `Tick`) and `view` (`let (w, h) = c.size(); let th = cx.colors(); let m = cx.metrics(); let g = m.gutter();` then widgets).

## Several panes with tabs

`enum Pane { Home, Notes, Files }` in the state; a tab strip of `widgets::button` (the shown pane `primary = true`); `view` draws the strip then the pane in the rectangle below. File: `recipes/add_tabs.rs`. Each pane that embeds a view: route `Action`, `Key`, `Text`, `Wheel` to that view only while its pane is shown; always call `tick()` of a view you keep alive.

## List and detail

`recipes/list_selection.rs`: rows `A_ROW0 + i`, arrow keys, detail beside the list on Medium and Expanded, list alone on Compact.

## Form

`recipes/form_validation.rs`: a `widgets::field` (focus on its action, `wants_keyboard()` true while focused), a validation function returning `Result<(), &str>`, the sentence of the error drawn under the field.

## Persistence

`recipes/persist_storage.rs`: read in the constructor, write in `event`, show a refusal.

## Embedded view

`recipes/embed_files.rs` (and chat, terminal, editor, audio, camera, browser): constructor takes the port as `Box<dyn ...Port>`; `Tick` -> `view.tick()`; `Action(a) if view.owns(a)` -> `view.action(a)`; `Key`/`Text`/`Wheel` -> the view; `draw` in a rectangle inside the pane.

## Custom drawing

`recipes/themed_drawing.rs`: `widgets::surface` for cards, `c.rrect` with `m.tokens.radius_for(Radius::Sm, h)`, colours from `th`.
