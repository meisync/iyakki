use iyakki_ui_sdk::assistant::ToolSpec;

/// An approval card is not drawn by the app: it comes from the core. Mark a tool that changes data as `ask = true` and the ChatView draws
/// the card (Allow and Deny, also `y` and `n`) and the core writes the audit line. A read-only tool is `ask = false`.
pub fn tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec::new("delete_note", "Delete one note by its number.", r#"{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]}"#, true),
        ToolSpec::new("list_notes", "List the notes.", r#"{"type":"object"}"#, false),
    ]
}
