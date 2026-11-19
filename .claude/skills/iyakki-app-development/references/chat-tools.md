# A chat tool for every switch

Rule (owner, 2026-11-06): whatever a person does on a screen, the assistant can do through a tool. Reads are free; changes ask every time.

## Pattern

1. One function does the work: `fn set_compact(&mut self, on: bool) -> Result<String, String>` (it saves through the storage port and returns a sentence).
2. The screen calls it: a `widgets::button` named "Compact rows" on `Action(A_COMPACT)`.
3. Two tools in the ChatView's `SessionConfig.tools`:
   `ToolSpec::new("compact_status", "Say whether rows are compact.", r#"{"type":"object"}"#, false)` and
   `ToolSpec::new("compact_set", "Turn compact rows on or off.", r#"{"type":"object","properties":{"on":{"type":"boolean"}},"required":["on"]}"#, true)`.
4. In the `tick()` loop: `for inv in chat.tick() { let r = match inv.tool.as_str() { "compact_status" => Ok(...), "compact_set" => { let a = Args::parse(&inv.args)?; self.set_compact(a.bool("on").ok_or("give on")?) } _ => Err("unknown tool".into()) }; chat.answer(&inv, r); }`
5. Test: script `SharedAssistant` with `Turn::call("", "compact_set", "{\"on\":true}")`; assert nothing changed before Allow; tap "Allow"; assert the value and the stored key. Assert the tool's `ask` is true (see `docs_recipes.rs`).
6. Description text is shown to the person on the card: write it plainly and name the effect.

## What stays with the owner

The wording of an approval card for a core action, which actions are "hard" (asked every time, never covered by a session grant), and any row in `docs/development/CHAT-REACHABILITY.md` for an OS Settings page. If your change needs one of those, stop and ask.

## Arguments: strings, whole numbers and booleans only

`Args::str`, `Args::uint`, `Args::bool` are all there is; an array or object argument arrives as raw text with no accessor. Pass a list as ONE string (`"SKU:QTY,SKU:QTY"`), describe the format in the tool description, and parse it with a function that answers a sentence per refusal: `docs/sdk/recipes/tool_list_args.rs` (tested).
