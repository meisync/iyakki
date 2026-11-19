//! Types moved out of `iyakki_agent::pdfnativetools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nPDF OUTPUT SWITCH: pdf_output_status lists which providers and agents are marked as writing PDF files themselves (then pdf_create is not offered to them); pdf_output_set changes one after the person approves (a provider id, or agent:claude, agent:copilot, agent:custom:<id>).";
