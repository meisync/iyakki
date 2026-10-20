//! Guards that keep a small model honest about what it can do.
//!
//! * [`detect_denial`]: the model's final text says it cannot do something (browse, take a screenshot, check the weather) while
//!   a tool that does exactly that exists in the registry. The loop then retries once with a corrective note naming the tools.
//!   A refusal that is true (browser control is off for the account, the engine is not installed, nothing suitable exists) is not
//!   touched.
//! * [`detect_ask_first`]: "Would you like me to fetch...?" for a read-only action the assistant is allowed to do on its own.
//! * [`condense`]: what is sent to the model from an old conversation: old refusals contradicted by today's tools, old failed
//!   tool calls and old bulky tool output are dropped or shortened (the saved transcript keeps everything), and a long history is
//!   cut to a token cap with the cut part reduced to a short rolling summary.
//! * [`LoopGuard`]: the same tool with the same arguments three times is a loop.

use std::collections::HashMap;

use crate::agent::estimate_tokens;
use crate::chat::{Message, Role};

/// What the guard found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    /// The tools (present in the registry) the model should use.
    pub tools: Vec<String>,
    /// What it was about, for the note shown in the chat ("web", "screenshot", ...).
    pub topic: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingKind {
    Denial,
    AskFirst,
}

/// topic, words (in the request or the refusal) that mean it, tools that do it (the first ones present are named).
const INTENTS: &[(&str, &[&str], &[&str])] = &[
    ("screenshot", &["screenshot", "screen shot", "picture of my screen", "my screen", "your interface", "own interface", "the screen"], &["screen_capture", "browser_screenshot"]),
    ("weather", &["weather", "temperature", "forecast", "rain", "humidity"], &["weather", "web_research"]),
    ("stock", &["stock", "share price", "price of", "nifty", "sensex", "nasdaq", "crypto", "bitcoin", "mutual fund", "ticker", "market"], &["stock_quote", "web_research", "web_search"]),
    ("news", &["news", "headline", "latest", "happening", "current events", "today's"], &["web_research", "web_feed", "web_search", "web_fetch"]),
    ("radio", &["radio", " fm", "fm ", "station"], &["radio_play", "radio_search"]),
    ("music", &["play ", "song", "music", "video", "playlist"], &["media_play", "media_search", "play_media", "radio_play"]),
    ("calendar", &["remind", "reminder", "calendar", "schedule", "alarm", "appointment", "task"], &["calendar_add", "alarm_set", "task_add"]),
    ("files", &["file", "folder", "document", "read my", "directory"], &["read_file", "list_dir", "search_documents"]),
    ("page", &["this page", "the page", "open tab", "on my screen in the browser"], &["browser_page", "browser_snapshot"]),
    ("web", &["website", "web site", "internet", "online", "url", "http", "www.", "gemini", "chatgpt", "google", "browse", "browser", "link", "site", "external service", "external website", "web "], &["web_research", "web_search", "web_fetch", "browser_open"]),
];

/// Phrases of a refusal ("cannot", "can't", ...) in lower case with straight apostrophes.
const REFUSAL: &[&str] = &[
    "cannot access", "can't access", "can not access", "unable to access", "not able to access", "don't have access", "do not have access", "no access to", "without access to",
    "cannot browse", "can't browse", "unable to browse", "cannot visit", "can't visit", "cannot open", "can't open", "unable to open",
    "cannot check", "can't check", "cannot fetch", "can't fetch", "cannot retrieve", "can't retrieve", "unable to retrieve", "unable to fetch", "cannot look up", "can't look up",
    "cannot take screenshots", "can't take screenshots", "cannot take a screenshot", "can't take a screenshot", "unable to take a screenshot", "unable to take screenshots", "cannot capture", "can't capture", "cannot see your screen", "can't see your screen",
    "cannot interact with", "can't interact with", "unable to interact with",
    "cannot connect to", "can't connect to", "unable to connect",
    "cannot provide real-time", "can't provide real-time", "no real-time", "don't have real-time", "do not have real-time", "unable to provide real-time", "cannot give you real-time",
    "don't have internet", "do not have internet", "don't have direct", "do not have direct", "no direct access", "have browser access", "have web access", "have network access", "have internet access", "have access to the internet", "have access to external", "no access to the internet", "no browser access", "no internet access", "not connected to the internet", "i'm offline", "i am offline",
    "my tools are limited", "my abilities are limited", "limited to the tools", "confined to the tools", "i only have access to", "i don't have any tool", "i do not have any tool", "i have no tool",
    "cannot play", "can't play", "cannot set a reminder", "can't set a reminder", "cannot set reminders", "can't set reminders",
];

/// Phrases that make a refusal true or already explained: the account's policy, a missing engine, a denied approval.
const LEGIT: &[&str] = &[
    "browser_status", "turned off", "is off for your account", "not enabled for your account", "not installed", "full web engine", "administrator", "not allowed to use", "you denied", "was denied", "did not answer", "approval", "not available on this", "policy", "needs to be enabled", "not permitted",
];

fn norm(s: &str) -> String {
    s.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn present(tools: &[&str], names: &[&str]) -> Vec<String> {
    tools.iter().filter(|t| names.contains(t)).map(|t| (*t).to_string()).collect()
}

/// Intent topics suggested by `request` and `reply`, with the tools present in the registry for each. The request counts most:
/// topics from it are listed first.
fn topics(request: &str, reply: &str, names: &[&str]) -> Vec<(&'static str, Vec<String>)> {
    let (rq, rp) = (format!(" {} ", norm(request)), format!(" {} ", norm(reply)));
    let mut out: Vec<(&'static str, Vec<String>)> = Vec::new();
    for pass in [&rq, &rp] {
        for (topic, words, tools) in INTENTS {
            if out.iter().any(|(t, _)| t == topic) {
                continue;
            }
            if words.iter().any(|w| pass.contains(w)) {
                let t = present(tools, names);
                if !t.is_empty() {
                    out.push((topic, t));
                }
            }
        }
    }
    out
}

/// The tools meant by a request when it is a plain one ("tamil fm", "stock price of envx"): used to name tools in notes.
pub fn tools_for_request(request: &str, names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for (_, t) in topics(request, "", names) {
        for x in t {
            if !v.contains(&x) {
                v.push(x);
            }
        }
    }
    v
}

/// A final answer that says "I cannot" although a tool does it. `None` for true refusals and ordinary answers.
pub fn detect_denial(reply: &str, request: &str, names: &[&str]) -> Option<Finding> {
    let r = norm(reply);
    // an answer with real content is not a refusal; a long text that mentions "I can't access X" in passing is left alone
    if r.is_empty() || r.len() > 900 {
        return None;
    }
    if !REFUSAL.iter().any(|p| r.contains(p)) {
        return None;
    }
    if LEGIT.iter().any(|p| r.contains(p)) {
        return None;
    }
    let found = topics(request, reply, names);
    // a refusal about the web needs the real web tools: a browser_status stub is not "a tool that does it"
    let (topic, tools) = found.into_iter().next()?;
    Some(Finding { kind: FindingKind::Denial, tools: tools.into_iter().take(4).collect(), topic })
}

const ASK_OPENERS: &[&str] = &["would you like me to ", "do you want me to ", "shall i ", "should i ", "want me to ", "would you like to have me ", "if you'd like, i can ", "if you like, i can ", "i can fetch", "i can search", "i can look", "let me know if you'd like me to ", "let me know if you want me to "];
const READ_VERBS: &[&str] = &["fetch", "search", "look up", "look for", "look at", "open", "check", "read", "find", "get ", "pull", "browse", "visit", "retrieve", "summar", "research", "load"];
const WRITE_VERBS: &[&str] = &["send", "delete", "remove", "buy", "pay", "post ", "install", "purchase", "write", "save", "overwrite", "submit", "email", "book", "order", "sign up", "log in", "login", "upload", "create"];

/// "Would you like me to fetch...?": a question that asks permission for a read-only lookup the assistant may just do.
pub fn detect_ask_first(reply: &str, request: &str, names: &[&str]) -> Option<Finding> {
    let r = norm(reply);
    if r.is_empty() || r.len() > 700 {
        return None;
    }
    let tail_start = r.len().saturating_sub(260);
    let tail = &r[r.char_indices().map(|(i, _)| i).find(|i| *i >= tail_start).unwrap_or(0)..];
    let opener = ASK_OPENERS.iter().find_map(|o| tail.find(o).map(|i| (o, i)))?;
    let after = &tail[opener.1 + opener.0.len()..];
    let clause: String = after.chars().take(70).collect();
    if !READ_VERBS.iter().any(|v| clause.contains(v)) || WRITE_VERBS.iter().any(|v| clause.contains(v)) {
        return None;
    }
    let mut found = topics(request, reply, names);
    if found.is_empty() {
        let generic = present(&["web_research", "web_search", "web_fetch"], names);
        if generic.is_empty() {
            return None;
        }
        found.push(("web", generic));
    }
    let (topic, tools) = found.into_iter().next()?;
    Some(Finding { kind: FindingKind::AskFirst, tools: tools.into_iter().take(4).collect(), topic })
}

/// The system note sent with the retry.
pub fn corrective_note(f: &Finding) -> String {
    let tools = f.tools.join(", ");
    match f.kind {
        FindingKind::Denial => format!("SYSTEM NOTE: your last reply said you cannot do this, but you can. These tools exist right now and do it: {tools}. Call the best one now, without asking first, then answer from its result. If the tool returns an error, quote that error exactly instead of saying you have no access."),
        FindingKind::AskFirst => format!("SYSTEM NOTE: do not ask permission for read-only lookups; reading needs no approval. Call {tools} now and answer from the result."),
    }
}

/// The line shown in the chat when the guard steps in.
pub fn retry_note(f: &Finding) -> String {
    match f.kind {
        FindingKind::Denial => format!("The model said it cannot do this, but these tools can ({}). Asking it again with the tool list.", f.tools.join(", ")),
        FindingKind::AskFirst => format!("The model asked permission for a lookup it may just do. Asking it to go ahead ({}).", f.tools.join(", ")),
    }
}

pub const WEAK_MODEL_NOTE: &str = "This model may not be using its tools well; try another model (Ctrl+P) or a new chat (Ctrl+N).";

// ------------------------------------------------------------------------------------------------ history hygiene

/// The marker that replaces an old refusal in what the model sees.
pub const WITHDRAWN: &str = "[An earlier answer here wrongly said this could not be done; it is left out. The tools listed under TOOLS AVAILABLE NOW can do it.]";
const CHECK_TOOL: &str = "assistant_check";

/// Result of [`condense`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Condensed {
    pub history: Vec<Message>,
    /// Rolling summary of what was cut (previous summary included), to store in the session. Empty if nothing was cut.
    pub summary: String,
    pub refusals_dropped: usize,
    pub errors_dropped: usize,
    pub results_shortened: usize,
    pub turns_summarised: usize,
}

fn first_line(s: &str, n: usize) -> String {
    let l = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let t: String = l.chars().take(n).collect();
    if l.chars().count() > n { format!("{t}…") } else { t }
}

/// One line per cut exchange, the model-free fallback for a rolling summary.
pub fn extractive_summary(cut: &[Message], previous: &str) -> String {
    let mut lines: Vec<String> = previous.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    let mut asked: Option<String> = None;
    for m in cut {
        match m.role {
            Role::User => asked = Some(first_line(&m.content, 60)),
            Role::Assistant if m.tool_calls.is_empty() => {
                let a = first_line(&m.content, 70);
                if a.starts_with("[An earlier answer") {
                    continue;
                }
                lines.push(format!("- asked: {} → {}", asked.take().unwrap_or_else(|| "?".into()), a));
            }
            _ => {}
        }
    }
    if let Some(q) = asked {
        lines.push(format!("- asked: {q}"));
    }
    // keep the newest lines within about 1500 characters
    let mut keep: Vec<String> = Vec::new();
    let mut size = 0;
    for l in lines.into_iter().rev() {
        size += l.len() + 1;
        if size > 1500 {
            break;
        }
        keep.push(l);
    }
    keep.reverse();
    keep.join("\n")
}

/// Prepare an old conversation for the model. `history` starts with the system message and ends with the new request.
/// `names` are the tools that exist now; `cap_tokens` (0 = no cap) bounds the whole request; `previous_summary` is the
/// session's rolling summary so far.
pub fn condense(history: Vec<Message>, names: &[&str], cap_tokens: u64, previous_summary: &str) -> Condensed {
    let mut c = Condensed { summary: previous_summary.to_string(), ..Default::default() };
    // index of the last user message = the new request; everything before it is "old"
    let last_user = history.iter().rposition(|m| m.role == Role::User).unwrap_or(0);
    // old turns: the two user turns before the new one keep their tool results in full
    let users: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == Role::User).map(|(i, _)| i).collect();
    let recent_from = users.iter().rev().nth(2).copied().unwrap_or(0);

    let mut out: Vec<Message> = Vec::with_capacity(history.len());
    let mut dropped_ids: Vec<String> = Vec::new();
    let mut prev_user = String::new();
    for (i, m) in history.into_iter().enumerate() {
        if i >= last_user {
            out.push(m);
            continue;
        }
        match m.role {
            Role::User => {
                prev_user = m.content.clone();
                out.push(m);
            }
            Role::Assistant if !m.tool_calls.is_empty() => {
                // the guard's own check steps are not part of the conversation
                if m.tool_calls.iter().all(|t| t.name == CHECK_TOOL) {
                    dropped_ids.extend(m.tool_calls.iter().map(|t| t.id.clone()));
                    continue;
                }
                out.push(m);
            }
            Role::Assistant => {
                if !m.content.starts_with("[An earlier answer") && detect_denial(&m.content, &prev_user, names).is_some() {
                    c.refusals_dropped += 1;
                    out.push(Message::assistant(WITHDRAWN));
                } else {
                    out.push(m);
                }
            }
            Role::Tool => {
                if m.tool_call_id.as_ref().is_some_and(|id| dropped_ids.contains(id)) {
                    continue;
                }
                let old = i < recent_from;
                if m.content.starts_with("Error:") && old {
                    c.errors_dropped += 1;
                    out.push(Message { content: "Error: (an old failure, left out)".into(), ..m });
                } else if old && m.content.chars().count() > 600 {
                    c.results_shortened += 1;
                    let head: String = m.content.chars().take(500).collect();
                    out.push(Message { content: format!("{head}\n[older tool result shortened]"), ..m });
                } else {
                    out.push(m);
                }
            }
            Role::System => out.push(m),
        }
    }

    // the cap: cut whole exchanges from the front (after the system message) until the request fits
    if cap_tokens > 0 && estimate_tokens(&out) > cap_tokens {
        let first = usize::from(out.first().is_some_and(|m| m.role == Role::System));
        let mut cut_to = first;
        let last = out.len().saturating_sub(1);
        let mut tokens = estimate_tokens(&out);
        // a boundary is a user message; never cut the newest request or tool results from their call
        while tokens > cap_tokens && cut_to < last {
            let next = (cut_to + 1..=last).find(|&j| out[j].role == Role::User).unwrap_or(last);
            if next >= last {
                break;
            }
            tokens = tokens.saturating_sub(estimate_tokens(&out[cut_to..next]));
            cut_to = next;
        }
        if cut_to > first {
            let cut: Vec<Message> = out.drain(first..cut_to).collect();
            c.turns_summarised = cut.iter().filter(|m| m.role == Role::User).count();
            c.summary = extractive_summary(&cut, &c.summary);
        }
    }
    if !c.summary.is_empty() && (c.turns_summarised > 0 || !previous_summary.is_empty()) {
        if let Some(sys) = out.first_mut().filter(|m| m.role == Role::System) {
            sys.content.push_str(&format!("\n\nSummary of the earlier part of this conversation (older messages are not repeated):\n{}", c.summary));
        }
    }
    c.history = out;
    c
}

/// Shrink the oldest tool results of a run in progress when the request outgrows the cap. Returns how many were shortened.
pub fn shrink_run_results(history: &mut [Message], cap_tokens: u64, keep_last: usize) -> usize {
    if cap_tokens == 0 || estimate_tokens(history) <= cap_tokens {
        return 0;
    }
    let tool_idx: Vec<usize> = history.iter().enumerate().filter(|(_, m)| m.role == Role::Tool).map(|(i, _)| i).collect();
    let mut n = 0;
    for &i in tool_idx.iter().take(tool_idx.len().saturating_sub(keep_last)) {
        if history[i].content.chars().count() > 400 {
            let head: String = history[i].content.chars().take(300).collect();
            history[i].content = format!("{head}\n[shortened to keep the request small]");
            n += 1;
        }
        if estimate_tokens(history) <= cap_tokens {
            break;
        }
    }
    n
}

// ------------------------------------------------------------------------------------------------ loops

/// Counts how often the same call (tool and arguments) gave the same answer in a row.
#[derive(Default)]
pub struct LoopGuard {
    seen: HashMap<String, (u32, u64)>,
}

impl LoopGuard {
    /// Record a call and its result; returns how many times in a row this exact call has now given this exact result
    /// (1 for the first time or when the result changed).
    pub fn note(&mut self, name: &str, args: &serde_json::Value, result: &str) -> u32 {
        use std::hash::{Hash, Hasher};
        // counters, timestamps and attempt numbers do not make two calls or two results different
        let mut h = std::collections::hash_map::DefaultHasher::new();
        crate::boardtrack::normalise(&result.chars().take(400).collect::<String>()).hash(&mut h);
        let digest = h.finish();
        let e = self.seen.entry(format!("{name}\u{1}{}", crate::boardtrack::normalise(&args.to_string()))).or_insert((0, digest));
        if e.1 == digest && e.0 > 0 {
            e.0 += 1;
        } else {
            *e = (1, digest);
        }
        e.0
    }
}

pub const LOOP_AT: u32 = 3;

// ------------------------------------------------------------------------------------------------ the stall guard

/// Most nudges one run may send.
pub const MAX_NUDGES: usize = 2;

/// What the model is told when it said it would act and then stopped. Request-only, never shown as a person's message.
pub const NUDGE: &str = "SYSTEM NOTE: you said you would act, but you made no tool call. Do it now with a tool call, or say plainly what blocks you and what you need from the person.";

const ANNOUNCES: &[&str] = &["let me", "i'll ", "i will try", "i will ", "let's", "lets ", "next, i", "next i", "now i", "now, i", "try again", "a different approach", "another approach", "i'll try", "i am going to", "i'm going to", "i need to first", "first, i"];
const BLOCKED_OR_DONE: &[&str] = &["i cannot", "i can't", "i can not", "i'm unable", "i am unable", "unable to", "not possible", "blocked", "i need you", "i need to know", "i need the", "need your", "permission", "please provide", "please tell", "please let me know", "let me know", "could you", "can you ", "would you like", "do you want", "shall i", "which one", "successfully", "has been created", "have been created", "is done", "all done", "all set", "completed", "here is", "here's", "here are", "summary"];

/// The model ended its turn with text and no tool call: did it only announce what it would do (or stop right after a failed
/// tool) without asking the person anything or reporting that it is finished or blocked?
/// `last_failed` is whether the tool result just before this turn was an error.
pub fn detect_stall(text: &str, last_failed: bool) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > 700 {
        return false;
    }
    let l = t.to_lowercase().replace('\u{2019}', "'");
    // a real question to the person (the last sentence ends in a question mark) is the person's turn
    if t.ends_with('?') || t.ends_with("?\"") || t.ends_with("?)") {
        return false;
    }
    let padded = format!(" {l}");
    if BLOCKED_OR_DONE.iter().any(|p| padded.contains(&format!(" {p}"))) {
        return false;
    }
    // after a failed tool an announcement anywhere in the text is a stall; otherwise only one in its closing words
    let skip = if last_failed { 0 } else { l.chars().count().saturating_sub(160) };
    let l: String = format!(" {}", l.chars().skip(skip).collect::<String>());
    ANNOUNCES.iter().any(|p| l.contains(&format!(" {p}")) || l.contains(&format!("\n{p}")))
}

/// Added to the system message of a request while approvals are automatic (Auto / Autopilot).
pub const AUTO_RULE: &str = "\n\nApprovals are automatic: do not ask whether to proceed, check, verify or continue; do it with a tool. Ask only when you are missing information or there is a real choice, using ask_user.";

/// The nudge for a permission-style closing question in auto mode.
pub const NUDGE_AUTO: &str = "SYSTEM NOTE: approvals are automatic. Do it now; do not ask. Call the tool for what you offered to do, or finish without a question.";

/// The nudge for Autopilot when the model answered with advice and no tool call while rows are open.
pub const NUDGE_ACT: &str = "SYSTEM NOTE: Autopilot is working and nothing has been done yet in this step. Do not describe or advise: make the change now with a tool call (write_file, deploy_build, deploy_load, k8s_apply, run_command), or say plainly what blocks you.";

const ADVICE: &[&str] = &["you should", "you can ", "you could", "you need to", "i recommend", "i suggest", "we should", "to fix", "to resolve", "consider ", "try to ", "try running", "should be changed", "needs to be", "need to be", "make sure", "ensure that", "kubectl ", "docker ", "podman ", "helm ", "update the", "change the", "modify the", "set the", "add the"];

/// Does the answer propose actions (advice, commands) rather than report results?
pub fn proposes_action(text: &str) -> bool {
    let l = text.to_lowercase();
    !l.trim_end().ends_with('?') && ADVICE.iter().any(|a| l.contains(a))
}

/// The nudge when the model asks the person to edit a file or run a command that it can do itself.
pub const NUDGE_DIY: &str = "SYSTEM NOTE: you have the tools to do this. Do it yourself now with the tools; do not ask the person to edit or run anything. Edit with write_file or run the command with run_command, deploy_build or k8s_apply, then re-run the failing step.";

const DELEGATE: &[&str] = &[
    "please open", "please edit", "please replace", "please update", "please change", "please add", "please create", "please save", "please run", "please execute", "please modify", "please rename", "please delete", "please apply", "please try",
    "open the dockerfile", "open the file", "open your ", "replace it with", "replace the line", "replace the content", "replace the following", "replace the dockerfile", "update your ", "modify your ", "edit your ", "change your ",
    "in your dockerfile", "in your manifest", "in your file", "try the following", "run the following", "run this command", "execute the following", "execute this command", "copy and paste", "copy the following", "copy this ",
    "make the following change", "apply the following change", "save the file", "save it as", "create a file named", "create a new file", "paste the following", "after that, run",
];
const MODAL: &[&str] = &["you need to ", "you should ", "you must ", "you have to ", "you will need to ", "you'll need to ", "you can then ", "you'd need to "];
const OBJECTS: &[&str] = &["file", "dockerfile", "manifest", "line ", "command", "yaml", "script", "package.json", "config", "code", "deployment.yaml", "makefile"];

/// Does the answer ask the person to open, edit, replace, create or save a file, or to run a command (including the pseudo call
/// `deploy_build(dir=".")` printed as text), when the assistant has tools that do it? A real question (it ends with `?`) is left alone.
pub fn asks_person_to_act(text: &str, tool_names: &[&str]) -> bool {
    if !tool_names.iter().any(|n| matches!(*n, "write_file" | "run_command" | "deploy_build" | "k8s_apply")) {
        return false;
    }
    let l = text.to_lowercase().replace('\u{2019}', "'");
    if l.trim_end().ends_with('?') {
        return false;
    }
    if DELEGATE.iter().any(|p| l.contains(p)) {
        return true;
    }
    for m in MODAL {
        let mut from = 0;
        while let Some(i) = l[from..].find(m) {
            let at = from + i + m.len();
            let window: String = l[at..].chars().take(90).collect();
            if OBJECTS.iter().any(|o| window.contains(o)) {
                return true;
            }
            from = at;
        }
    }
    // a tool call printed as text: `deploy_build(dir=".")`
    tool_names.iter().any(|n| n.contains('_') && l.contains(&format!("{n}(")))
}

const PERMISSION: &[&str] = &["would you like me to", "do you want me to", "want me to", "shall i", "should i proceed", "should i check", "should i verify", "should i continue", "should i go ahead", "should i run", "should i test", "should i start", "should i deploy", "let me know if you want me", "let me know if you would like me", "let me know if i should", "can i go ahead", "is it ok if i", "is it okay if i", "would you like to proceed"];
const DOABLE: &[&str] = &["check", "verify", "test", "run", "start", "deploy", "continue", "proceed", "apply", "build", "load", "fix", "update", "create", "write", "open", "look", "retry", "try", "restart", "go ahead", "help", "see", "confirm", "make"];
const NEEDS_INFO: &[&str] = &["which", "what ", "prefer", "port", "name", "version", "password", "token", "choose", "between", "option", "instead of", "delete", "remove", "uninstall", "destroy", "drop ", "sudo", "secret", "vault", "credential", "browser", "sign in", "log in", "overwrite"];

/// In auto mode: does the closing sentence only ask permission to do something the assistant can do itself ("Would you like
/// me to check if the service is responding?"), with no missing information, no real choice and nothing that always asks?
pub fn asks_permission(text: &str) -> bool {
    let t = text.trim();
    let tail: String = t.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
    let l = tail.to_lowercase().replace('\u{2019}', "'");
    // the last sentence only
    let cut = l.trim_end_matches(['?', '.', '!', ' ']).rfind(['.', '!', '\n', ':']).map_or(0, |i| i + 1);
    let last = l[cut..].trim();
    if !(t.ends_with('?') || last.contains("let me know")) {
        return false;
    }
    PERMISSION.iter().any(|p| last.contains(p)) && DOABLE.iter().any(|a| last.contains(a)) && !NEEDS_INFO.iter().any(|n| last.contains(n))
}

/// Does a final answer claim that work was done or is under way (so open board rows contradict it)?
pub fn claims_progress(text: &str) -> bool {
    let l = text.to_lowercase();
    if l.trim_end().ends_with('?') {
        return false;
    }
    ["done", "complete", "created", "built", "deployed", "running", "successfully", "ready", "finished", "implemented", "set up", "written", "installed", "works", "working"].iter().any(|w| l.contains(w))
}

/// The request-only note that asks the model to settle open board rows.
pub fn board_nudge(open: &[String]) -> String {
    let shown: Vec<&str> = open.iter().take(8).map(String::as_str).collect();
    format!("SYSTEM NOTE: the board still lists {} as open{}. For each one call board_update (done only if a test or check really passed; partial or blocked with a comment saying why), or say plainly why it is not finished. Do not repeat the summary.", shown.join(", "), if open.len() > 8 { " and more" } else { "" })
}

/// True when the last `n` tool results of this run are the same error: the model is repeating a failing call and a nudge
/// would only feed the loop.
pub fn repeated_failures(results: &[&str], n: usize) -> bool {
    results.len() >= n && results[results.len() - n..].iter().all(|r| r.starts_with("Error:") && *r == results[results.len() - 1])
}

// ------------------------------------------------------------------------------------------------ questions answered by a click

/// A question the assistant ended its message with, and the answers the chat offers as buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub question: String,
    /// One answer per button, exactly as it is sent when clicked. Empty for a yes/no question (the chat adds Yes and No).
    pub options: Vec<String>,
    pub yes_no: bool,
}

impl Ask {
    /// The buttons: Yes and No, or the options.
    pub fn answers(&self) -> Vec<String> {
        if self.yes_no || self.options.is_empty() {
            vec!["Yes".into(), "No".into()]
        } else {
            self.options.clone()
        }
    }
}

/// Phrases that make a final question one the person answers with a short choice.
const QUESTION_CUES: &[&str] = &[
    "would you like", "shall i", "do you want", "should i", "want me to", "which", "proceed", "confirm", "may i", "can i", "is that ok", "is that okay", "ok to ", "okay to ", "go ahead",
    "would you prefer", "do you prefer", "prefer", "is this ok", "are you sure",
];
const CHOICE_WORDS: &[&str] = &["which", "prefer", "choose", "pick", "option", "select", "one of", "number"];
const OPEN_WORDS: &[&str] = &["what", "where", "when", "how", "who", "why", "whose"];

/// The tool call `ask_user {question, options[]}` as an [`Ask`] (`None` without a question).
pub fn ask_from_args(args: &serde_json::Value) -> Option<Ask> {
    let question = args.get("question").and_then(|v| v.as_str())?.trim().to_string();
    if question.is_empty() {
        return None;
    }
    let mut options: Vec<String> = Vec::new();
    for o in args.get("options").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
        let o: String = o.trim().chars().take(80).collect();
        if !o.is_empty() && !options.iter().any(|x| x.eq_ignore_ascii_case(&o)) && options.len() < 9 {
            options.push(o);
        }
    }
    let yes_no = options.is_empty() || (options.len() == 2 && options.iter().any(|o| o.eq_ignore_ascii_case("yes")) && options.iter().any(|o| o.eq_ignore_ascii_case("no")));
    if yes_no {
        options.clear();
    }
    Some(Ask { question, options, yes_no })
}

/// The question as text in the chat, with the options numbered (so the transcript still reads right once the buttons are gone).
pub fn ask_text(a: &Ask) -> String {
    if a.options.is_empty() {
        return a.question.clone();
    }
    let list: Vec<String> = a.options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
    format!("{}\n\n{}", a.question, list.join("\n"))
}

/// The nudge when a finished goal still ends with a question.
pub const NUDGE_SUMMARY: &str = "SYSTEM NOTE: the goal is complete. Write the summary only: what was done, what is blocked, files changed, commands run. Do not ask anything, offer options or write \"would you like\" or \"please choose\".";

const QUESTION_MARKS: &[&str] = &["would you like", "do you want", "please choose", "please select", "please pick", "which option", "choose an option", "shall i", "should i", "let me know if you want", "what would you like"];

/// Does the text ask the person something (a question mark at the end of a paragraph, an offer, a request to choose)?
pub fn asks_anything(text: &str) -> bool {
    let l = text.to_lowercase();
    QUESTION_MARKS.iter().any(|q| l.contains(q)) || text.trim_end().ends_with('?') || text.split("\n\n").any(|p| p.trim_end().ends_with('?'))
}

/// The text without its closing question: paragraphs from the first one that offers options or asks, to the end, are dropped (the summary
/// before it stays).
pub fn strip_closing_question(text: &str) -> String {
    let paras: Vec<&str> = text.split("\n\n").collect();
    let cut = paras.iter().position(|p| { let l = p.to_lowercase(); QUESTION_MARKS.iter().any(|q| l.contains(q)) || p.trim_end().ends_with('?') });
    match cut {
        Some(i) => paras[..i].join("\n\n").trim_end().to_string(),
        None => text.to_string(),
    }
}

/// A turn's prose that only repeats the question `ask_user` asks (the question or at least two of its options): the repeated part is
/// dropped from the prose, so the person sees the question once, with its buttons.
pub fn strip_repeated_question(text: &str, ask: &Ask) -> String {
    let l = text.to_lowercase();
    let q = ask.question.to_lowercase();
    let opts_in = ask.options.iter().filter(|o| l.contains(&o.to_lowercase())).count();
    let repeats = l.contains(q.trim_end_matches('?')) || (ask.options.len() >= 2 && opts_in >= 2) || (ask.options.is_empty() && QUESTION_MARKS.iter().any(|m| l.contains(m)));
    if !repeats {
        return text.to_string();
    }
    strip_closing_question(text)
}

/// A bullet or numbered line: its text (cut at a dash or a colon, markup removed).
fn list_item(line: &str) -> Option<String> {
    let l = line.trim_start();
    let first = l.chars().next()?;
    let rest: &str = if matches!(first, '-' | '*' | '•') {
        l[first.len_utf8()..].strip_prefix(' ')?
    } else if first.is_ascii_digit() {
        let digits = l.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 2 {
            return None;
        }
        l[digits..].strip_prefix(['.', ')'])?.strip_prefix(' ')?
    } else if matches!(first, 'A'..='H' | 'a'..='h') {
        l[1..].strip_prefix(['.', ')'])?.strip_prefix(' ')?
    } else {
        return None;
    };
    let mut s = rest.replace("**", "").replace('`', "");
    for sep in [" — ", " – ", " - ", ": "] {
        if let Some(i) = s.find(sep) {
            if i > 0 {
                s.truncate(i);
            }
        }
    }
    let s: String = s.trim().trim_end_matches([':', ';', ',', '.']).trim().chars().take(80).collect();
    (!s.is_empty()).then_some(s)
}

fn plain(s: &str) -> String {
    s.replace("**", "").replace(['`', '_'], "").trim().to_string()
}

/// The last sentence of a line that ends with "?" (with the question mark).
fn last_sentence(line: &str) -> String {
    let body = plain(line);
    let body = body.trim_end_matches(['?', ' ', '"', '\'', ')', '”', '*']);
    let start = [". ", "! ", "? ", ".\" ", ".) "].iter().filter_map(|b| body.rfind(b).map(|i| i + b.len())).max().unwrap_or(0);
    let s = body[start..].trim().trim_start_matches(['-', '*', '•', ' ']);
    format!("{s}?")
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// "A or B", "A, B or C", "Would you like the short version or the detailed one?": the options.
fn or_options(q: &str) -> Option<Vec<String>> {
    let body = q.trim_end_matches('?').trim();
    let lower = body.to_ascii_lowercase();
    let tail: &str = if let Some(i) = body.rfind(':') {
        &body[i + 1..]
    } else {
        const PREFIXES: &[&str] = &["would you like me to ", "would you like to ", "would you like ", "do you want me to ", "do you want to ", "do you want ", "would you prefer ", "do you prefer ", "shall i ", "should i ", "want me to "];
        if let Some(i) = PREFIXES.iter().find_map(|p| lower.find(p).map(|i| i + p.len())) {
            &body[i..]
        } else {
            let i = [" prefer ", " want ", " like ", " choose ", " pick ", " use "].iter().filter_map(|w| lower.rfind(w).map(|i| i + w.len())).max()?;
            &body[i..]
        }
    };
    let joined = tail.replace(", or ", " or ").replace(" or ", ", ");
    let mut out: Vec<String> = Vec::new();
    for part in joined.split(',') {
        let p = part.trim().trim_matches(|c| matches!(c, '"' | '\'' | '“' | '”' | '(' | ')' | '.' | ' '));
        if p.is_empty() || p.split_whitespace().count() > 8 || p.chars().count() > 60 {
            return None;
        }
        let p = capitalise(p);
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
            out.push(p);
        }
    }
    (2..=6).contains(&out.len()).then_some(out)
}

/// Is the message a question the person answers with a short choice? Looks at the end of the message only: a last sentence
/// ending with "?" that asks permission or a choice ("Would you like...", "Shall I...", "Which...", "...or..."), with the
/// options from a numbered or bulleted list next to it or from the "A or B" in the sentence. A rhetorical question, a
/// question about a fact ("What is your name?") or a statement is not one.
pub fn detect_question(text: &str) -> Option<Ask> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 6000 {
        return None;
    }
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // fenced code at the end is not a question
    if lines.last()?.starts_with("```") {
        return None;
    }
    let mut k = 0;
    while k < lines.len() && list_item(lines[lines.len() - 1 - k]).is_some() {
        k += 1;
    }
    let ends_q = |l: &str| plain(l).trim_end_matches(['"', '\'', ')', '”', ' ']).ends_with('?');
    // the question, then its options as a list at the end
    if (2..=9).contains(&k) && k < lines.len() {
        let qline = lines[lines.len() - 1 - k];
        let q = last_sentence(qline);
        let ql = q.to_ascii_lowercase();
        let colon = plain(qline).ends_with(':');
        if (ends_q(qline) || colon) && QUESTION_CUES.iter().chain(CHOICE_WORDS).any(|c| ql.contains(c)) {
            let options: Vec<String> = lines[lines.len() - k..].iter().filter_map(|l| list_item(l)).collect();
            let q = if colon { format!("{}:", q.trim_end_matches('?')) } else { q };
            return Some(Ask { question: q, options, yes_no: false });
        }
        return None;
    }
    if k > 0 {
        return None;
    }
    let last = *lines.last()?;
    if !ends_q(last) {
        return None;
    }
    let q = last_sentence(last);
    if q.chars().count() > 300 || q.len() < 4 {
        return None;
    }
    let ql = q.to_ascii_lowercase();
    if !QUESTION_CUES.iter().any(|c| ql.contains(c)) {
        return None;
    }
    if OPEN_WORDS.iter().any(|w| ql.starts_with(&format!("{w} "))) {
        return None;
    }
    // options listed just above the question
    let mut j = lines.len() - 1;
    let mut above: Vec<String> = Vec::new();
    while j > 0 {
        match list_item(lines[j - 1]) {
            Some(i) => {
                above.push(i);
                j -= 1;
            }
            None => break,
        }
    }
    above.reverse();
    if (2..=9).contains(&above.len()) && CHOICE_WORDS.iter().any(|c| ql.contains(c)) {
        return Some(Ask { question: q, options: above, yes_no: false });
    }
    if ql.contains(" or ") || ql.contains(": ") {
        if let Some(options) = or_options(&q) {
            let yn = options.len() == 2 && options.iter().all(|o| o.eq_ignore_ascii_case("yes") || o.eq_ignore_ascii_case("no"));
            return Some(if yn { Ask { question: q, options: vec![], yes_no: true } } else { Ask { question: q, options, yes_no: false } });
        }
        if ql.contains(" or ") {
            return None;
        }
    }
    // a plain yes/no needs a cue that asks permission; "which" and "prefer" alone ask for a choice that is not named
    const YES_NO_CUES: &[&str] = &["would you like", "shall i", "do you want", "should i", "want me to", "proceed", "confirm", "may i", "can i", "is that ok", "ok to ", "okay to ", "go ahead", "is this ok", "are you sure"];
    if !YES_NO_CUES.iter().any(|c| ql.contains(c)) {
        return None;
    }
    Some(Ask { question: q, options: vec![], yes_no: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ToolCall;
    use serde_json::json;

    const ALL: &[&str] = &["web_research", "web_search", "web_fetch", "web_feed", "weather", "stock_quote", "browser_open", "browser_status", "browser_page", "browser_screenshot", "screen_capture", "radio_play", "radio_search", "media_play", "media_search", "calendar_add", "alarm_set", "read_file", "list_dir"];
    const POLICY_OFF: &[&str] = &["web_research", "web_search", "web_fetch", "weather", "stock_quote", "browser_status", "radio_play", "read_file"];

    fn tools(f: Option<Finding>) -> Option<Vec<String>> {
        f.map(|f| f.tools)
    }

    #[test]
    fn refusals_that_contradict_tools_are_found() {
        // the exact wording from the screenshots, and its relatives
        let cases: &[(&str, &str, &str)] = &[
            ("go to gemini website and ask something", "I cannot access external websites or services. My capabilities are confined to the tools described earlier (web research, text analysis, planning).", "browser_open"),
            ("check the stock price of envx", "I cannot check access to external services… My capabilities are confined to the tools described earlier.", "stock_quote"),
            ("take a screenshot of my screen", "I cannot take screenshots of my own interface.", "screen_capture"),
            ("what is the weather in chennai", "I don't have real-time data access, so I can't tell you the weather.", "weather"),
            ("top stock market news", "I don't have direct internet access.", "web_research"),
            ("open gemini and ask it a question", "I don't have browser access, sorry.", "browser_open"),
            ("show me the latest news", "I'm unable to access the internet to get news.", "web_research"),
            ("play a tamil fm", "As an AI, I can't play radio stations.", "radio_play"),
            ("remind me to call mom at 5", "I cannot set a reminder for you.", "calendar_add"),
            ("what's the weather", "I can’t access live information.", "weather"),
            ("summarise this site", "I cannot interact with external services.", "web_research"),
            ("what does the page say", "I'm unable to open web pages; my tools are limited to text analysis.", "browser_page"),
        ];
        for (req, reply, want) in cases {
            let got = tools(detect_denial(reply, req, ALL)).unwrap_or_else(|| panic!("not caught: {reply}"));
            assert!(got.iter().any(|t| t == want), "{reply}: {got:?} should contain {want}");
        }
    }

    #[test]
    fn true_refusals_and_ordinary_answers_are_left_alone() {
        // policy-off browser: only browser_status exists, and the model says so honestly
        let honest = [
            "I cannot browse in your tab: browser control is off for your account. An administrator can change that.",
            "The full web engine is not installed here, so I cannot open that page in the browser.",
            "I can't take screenshots: browser_status says browser control is unavailable.",
            "I cannot open it because you denied the approval request.",
            "The person did not answer the approval request, so I cannot continue; please approve and retry.",
        ];
        for r in honest {
            assert_eq!(detect_denial(r, "open gemini in my browser", POLICY_OFF), None, "{r}");
        }
        // no tool for the topic exists at all: nothing to correct
        assert_eq!(detect_denial("I cannot take screenshots of your screen.", "take a screenshot", POLICY_OFF), None);
        assert_eq!(detect_denial("I can't connect to the printer.", "print this", POLICY_OFF), None);
        // normal answers, including long ones that mention a limit in passing
        assert_eq!(detect_denial("Envx closed at 42.10 [1]. Prices may be delayed.", "stock price of envx", ALL), None);
        let long = format!("{} I cannot access paywalled sites. {}", "Here are the stories. ".repeat(40), "Sources follow. ".repeat(20));
        assert_eq!(detect_denial(&long, "top news", ALL), None);
        assert_eq!(detect_denial("", "x", ALL), None);
    }

    #[test]
    fn permission_asks_for_read_only_actions_are_found_but_writes_are_not() {
        let ask = [
            ("stock price of envx", "I can help with that. Would you like me to fetch the latest price for Envx?"),
            ("top stock market news", "Do you want me to search for the latest headlines?"),
            ("weather in chennai", "Shall I look up the current weather in Chennai?"),
            ("what does gemini.google.com say", "Would you like me to open the website and read it?"),
        ];
        for (req, reply) in ask {
            let f = detect_ask_first(reply, req, ALL).unwrap_or_else(|| panic!("not caught: {reply}"));
            assert_eq!(f.kind, FindingKind::AskFirst);
            assert!(!f.tools.is_empty());
        }
        for reply in ["Would you like me to send this email to your boss?", "Shall I delete the old backups?", "Do you want me to install kubectl?", "Should I buy the tickets now?", "I fetched it. Anything else?", "Would you like me to save that as notes.md?"] {
            assert_eq!(detect_ask_first(reply, "do it", ALL), None, "{reply}");
        }
        assert_eq!(detect_ask_first("Would you like me to fetch it?", "x", &["now"]), None, "no suitable tool: nothing to push for");
    }

    #[test]
    fn the_corrective_note_names_the_tools() {
        let f = detect_denial("I cannot access external websites.", "open the gemini website", ALL).unwrap();
        let n = corrective_note(&f);
        assert!(n.starts_with("SYSTEM NOTE") && n.contains("browser_open") && n.contains("without asking"), "{n}");
        assert!(retry_note(&f).contains("tools can"));
        let a = detect_ask_first("Would you like me to fetch the price?", "price of envx", ALL).unwrap();
        assert!(corrective_note(&a).contains("do not ask permission") && corrective_note(&a).contains("stock_quote"));
        assert!(WEAK_MODEL_NOTE.contains("Ctrl+P") && WEAK_MODEL_NOTE.contains("Ctrl+N"));
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), args: json!({}) }
    }

    fn chat_with_refusals() -> Vec<Message> {
        let c = call("c1", "web_fetch");
        let g = call("g1", CHECK_TOOL);
        vec![
            Message::system("SYS"),
            Message::user("go to gemini website"),
            Message::assistant("I cannot access external websites."),
            Message::user("check envx price"),
            Message::assistant_calls("", vec![g.clone()]),
            Message::tool_result(&g, "note"),
            Message::assistant_calls("", vec![c.clone()]),
            Message::tool_result(&c, format!("Error: {}", "blocked 403 ".repeat(100))),
            Message::assistant("Envx is 42 [1]."),
            Message::user("and the weather"),
            Message::assistant("Chennai 31 C."),
            Message::user("and the news"),
            Message::assistant("Here are headlines."),
            Message::user("take a screenshot"),
        ]
    }

    #[test]
    fn old_refusals_and_failed_tool_noise_are_left_out_of_the_request_only() {
        let original = chat_with_refusals();
        let c = condense(original.clone(), ALL, 0, "");
        assert_eq!(c.refusals_dropped, 1);
        assert_eq!(c.history[2].content, WITHDRAWN);
        assert!(!c.history.iter().any(|m| m.tool_calls.iter().any(|t| t.name == CHECK_TOOL)), "the guard's own steps are gone");
        assert!(!c.history.iter().any(|m| m.tool_call_id.as_deref() == Some("g1")));
        assert_eq!(c.errors_dropped, 1);
        assert!(c.history.iter().any(|m| m.content == "Error: (an old failure, left out)"));
        assert_eq!(c.history.last().unwrap().content, "take a screenshot");
        // pairing is intact: every tool message still follows its call
        for (i, m) in c.history.iter().enumerate() {
            if m.role == Role::Tool {
                assert!(c.history[i - 1].tool_calls.iter().any(|t| Some(&t.id) == m.tool_call_id.as_ref()));
            }
        }
        assert_eq!(original, chat_with_refusals(), "the input (the saved transcript) is not touched");
        // with no matching tool the refusal is true and stays
        let c = condense(chat_with_refusals(), &["browser_status", "read_file"], 0, "");
        assert_eq!(c.refusals_dropped, 0);
        assert!(c.history[2].content.starts_with("I cannot access"));
    }

    #[test]
    fn long_histories_are_cut_to_the_cap_with_a_rolling_summary() {
        let mut h = vec![Message::system("SYS")];
        for i in 0..40 {
            h.push(Message::user(format!("question number {i} {}", "padding ".repeat(30))));
            h.push(Message::assistant(format!("answer number {i} {}", "words ".repeat(30))));
        }
        h.push(Message::user("newest request"));
        let before = estimate_tokens(&h);
        let c = condense(h, ALL, 1500, "");
        let after = estimate_tokens(&c.history);
        assert!(before > 4000 && after <= 1900, "{before} -> {after}");
        assert_eq!(c.history[0].role, Role::System);
        assert!(c.history[0].content.contains("Summary of the earlier part") && c.history[0].content.contains("question number 2"), "{}", c.history[0].content);
        assert_eq!(c.history.last().unwrap().content, "newest request");
        assert_eq!(c.history[1].role, Role::User, "the window starts at a user message");
        assert!(c.turns_summarised > 20 && c.summary.len() <= 1600);
        // the summary is carried forward
        let c2 = condense(vec![Message::system("SYS"), Message::user("hi")], ALL, 1500, &c.summary);
        assert!(c2.history[0].content.contains("question number 2"));
    }

    #[test]
    fn old_bulky_tool_output_is_shortened_but_recent_output_is_kept() {
        let c1 = call("a", "web_fetch");
        let c2 = call("b", "web_fetch");
        let mut h = vec![Message::system("S"), Message::user("one"), Message::assistant_calls("", vec![c1.clone()]), Message::tool_result(&c1, "x".repeat(5000)), Message::assistant("ok")];
        for i in 0..3 {
            h.push(Message::user(format!("u{i}")));
            h.push(Message::assistant(format!("a{i}")));
        }
        h.push(Message::user("now"));
        h.push(Message::assistant_calls("", vec![c2.clone()]));
        h.push(Message::tool_result(&c2, "y".repeat(5000)));
        let c = condense(h, ALL, 0, "");
        assert_eq!(c.results_shortened, 1);
        assert!(c.history.iter().find(|m| m.tool_call_id.as_deref() == Some("a")).unwrap().content.len() < 700);
        assert_eq!(c.history.iter().find(|m| m.tool_call_id.as_deref() == Some("b")).unwrap().content.len(), 5000);
    }

    #[test]
    fn a_run_in_progress_shrinks_old_results_first() {
        let c1 = call("a", "web_fetch");
        let c2 = call("b", "web_fetch");
        let mut h = vec![Message::user("q"), Message::tool_result(&c1, "x".repeat(8000)), Message::tool_result(&c2, "y".repeat(8000))];
        let n = shrink_run_results(&mut h, 2500, 1);
        assert_eq!(n, 1);
        assert!(h[1].content.len() < 500 && h[2].content.len() == 8000);
        assert_eq!(shrink_run_results(&mut h, 0, 1), 0);
    }

    #[test]
    fn requests_to_the_person_to_edit_or_run_something_are_told_from_summaries_and_questions() {
        let tools = ["write_file", "run_command", "deploy_build", "now"];
        let reply = "Please, open the Dockerfile and replace it:\n```\nFROM node:14\nWORKDIR /app\n```\nAfter that, run the deploy_build command again. Try the following command: deploy_build(dir=\".\", image=\"web\")";
        assert!(asks_person_to_act(reply, &tools));
        for t in ["You need to edit the Dockerfile so that it has a USER line.", "You should change the manifest and add the image name.", "Run the following: kubectl get pods", "Copy and paste this into server.js:", "In your Dockerfile change line 1 to FROM node:18.", "Please update your package.json.", "Make the following changes to the file:", "```\ndeploy_build(dir=\".\")\n```"] {
            assert!(asks_person_to_act(t, &tools), "{t}");
        }
        // what it did, what it found, real questions, and a chat without file or command tools are left alone
        for t in ["I replaced line 1 of the Dockerfile with FROM node:18 and rebuilt the image; the test build now passes.", "The Dockerfile had no USER line. I added USER 10001 and redeployed.", "Which Node version should the Dockerfile use?", "You can reach the app at http://192.168.1.5:30080.", "Summary: 2 of 3 rows done. The build is green.", "Would you like me to replace the line?"] {
            assert!(!asks_person_to_act(t, &tools), "{t}");
        }
        assert!(!asks_person_to_act(reply, &["now", "weather"]), "no tools that could do it");
    }

    #[test]
    fn questions_after_a_finished_goal_and_prose_that_repeats_ask_user_are_cut() {
        let t = "All resources in test-cla-dev were removed and the namespace is gone.\n\nWould you like me to:\n1. Keep it running\n2. Stop it\n3. Remove all\n\nPlease choose an option.";
        assert!(asks_anything(t) && asks_anything("Done. Anything else?") && !asks_anything("Everything was removed. The namespace is gone."));
        assert_eq!(strip_closing_question(t), "All resources in test-cla-dev were removed and the namespace is gone.");
        assert_eq!(strip_closing_question("Summary only."), "Summary only.");
        let ask = ask_from_args(&serde_json::json!({"question": "Tear down the dev environment?", "options": ["Keep it running", "Stop it", "Remove all"]})).unwrap();
        // the prose repeats the options: it keeps the summary and loses the question
        assert_eq!(strip_repeated_question(t, &ask), "All resources in test-cla-dev were removed and the namespace is gone.");
        // prose that says something else stays whole
        let other = "I built the image and deployed it. It answers on port 3000.";
        assert_eq!(strip_repeated_question(other, &ask), other);
        assert_eq!(strip_repeated_question("Deployed.\n\nTear down the dev environment?", &ask), "Deployed.");
    }

    #[test]
    fn permission_questions_in_auto_mode_are_told_from_real_questions() {
        assert!(asks_permission("The service is deployed. Would you like me to check if the service is responding or help with anything else?"));
        assert!(asks_permission("Done with the manifest. Shall I apply it?"));
        assert!(asks_permission("Everything is built. Do you want me to verify the rollout?"));
        assert!(asks_permission("Built. Let me know if you want me to deploy it."));
        assert!(!asks_permission("Which port should the server listen on?"), "missing information");
        assert!(!asks_permission("Would you like me to use npm or yarn?"), "a real choice");
        assert!(!asks_permission("Would you like me to delete the old deployment?"), "destructive actions always ask");
        assert!(!asks_permission("I created server.js and the Dockerfile."), "no question");
        assert!(!asks_permission("Let me know if you need anything else."), "nothing the assistant could do");
        assert!(!asks_permission("Should I use the vault for the token?"));
    }

    #[test]
    fn progress_claims_and_the_board_note() {
        assert!(claims_progress("The server is running and the image was built."));
        assert!(!claims_progress("Should I deploy it now?"));
        assert!(!claims_progress("I need your decision on the port."));
        let n = board_nudge(&["F-APP-01 (planned)".into(), "F-APP-02 (partial)".into()]);
        assert!(n.contains("F-APP-01 (planned), F-APP-02 (partial)") && n.contains("board_update"));
    }

    #[test]
    fn stalls_are_told_from_answers_and_questions() {
        let said = "It seems there is an issue with the directory structure. Let me try a different approach to ensure the directory exists before writing the file.";
        assert!(detect_stall(said, true));
        assert!(detect_stall("Now I will write the manifest.", false), "an announced action is a stall even without an error");
        assert!(!detect_stall("The write failed: Error: refused.", true), "a short report after an error is an answer");
        assert!(!detect_stall("The file app.py now holds the server. It is done.", false));
        assert!(!detect_stall("Which folder should I use for the manifest?", true), "a question is the person's turn");
        assert!(!detect_stall("Let me know which folder to use.", true));
        assert!(!detect_stall("I cannot write there: the folder belongs to another user.", true), "blocked is reported, not nudged");
        assert!(!detect_stall("", true));
        assert!(!detect_stall(&"Let me explain. ".repeat(80), true), "a long answer is an answer");
        assert!(repeated_failures(&["Error: a", "Error: a", "Error: a"], 3));
        assert!(!repeated_failures(&["Error: a", "Error: b", "Error: a"], 3));
        assert!(!repeated_failures(&["ok", "Error: a", "Error: a"], 3));
    }

    #[test]
    fn the_loop_guard_counts_identical_calls() {
        let mut g = LoopGuard::default();
        let a = json!({"query": "x"});
        assert_eq!((g.note("web_search", &a, "r"), g.note("web_search", &a, "r"), g.note("web_search", &json!({"query": "y"}), "r"), g.note("web_search", &a, "r")), (1, 2, 1, 3));
        // the same call with a different answer each time (a page that changes) is not a loop
        assert_eq!((g.note("peek", &a, "alpha"), g.note("peek", &a, "beta"), g.note("peek", &a, "gamma")), (1, 1, 1));
        // but a counter or a timestamp in the text is not a different answer: the digits are ignored
        assert_eq!((g.note("check", &a, "The check failed 78 times"), g.note("check", &a, "The check failed 79 times"), g.note("check", &json!({"query": "x", "n": 12}), "The check failed 80 times")), (1, 2, 1), "arguments are normalised too: x with n=12 is another call");
        assert_eq!((g.note("check", &a, "The check failed 81 times"), g.note("check", &a, "The check failed 82 times")), (3, 4));
        assert_eq!(LOOP_AT, 3);
    }

    #[test]
    fn final_questions_become_answer_buttons_and_rhetorical_ones_do_not() {
        let yn = |t: &str| detect_question(t).unwrap_or_else(|| panic!("not detected: {t}"));
        for t in ["The build is still running. Would you like me to check the status again?", "Shall I proceed?", "I can delete the old logs. Do you want me to go ahead?", "Should I install it?", "**Shall I proceed?**", "Done with the plan. Please confirm: is this OK?"] {
            let a = yn(t);
            assert!(a.yes_no && a.answers() == vec!["Yes", "No"], "{t}: {a:?}");
        }
        assert_eq!(yn("Fixed it. Would you like me to check the status again?").question, "Would you like me to check the status again?");
        let a = yn("Which registry do you want to use: Docker Hub or GHCR?");
        assert_eq!(a.options, vec!["Docker Hub", "GHCR"]);
        assert!(!a.yes_no);
        assert_eq!(yn("Would you like the short version or the detailed one?").options, vec!["The short version", "The detailed one"]);
        assert_eq!(yn("Do you want A, B or C?").options, vec!["A", "B", "C"]);
        assert_eq!(yn("Shall I proceed or cancel?").options, vec!["Proceed", "Cancel"]);
        assert!(yn("Do you want yes or no?").yes_no);
        // numbered and bulleted lists, before or after the question
        let a = yn("There are three registries:\n\n1. Docker Hub: the default\n2. GHCR\n3. **Quay** - red hat\n\nWhich one should I use?");
        assert_eq!(a.options, vec!["Docker Hub", "GHCR", "Quay"]);
        let a = yn("Which would you like?\n- Short summary\n- Full report");
        assert_eq!((a.question.as_str(), a.options.clone()), ("Which would you like?", vec!["Short summary".to_string(), "Full report".to_string()]));
        let a = yn("Pick one:\nA) the fast way\nB) the safe way\n\nWhich do you prefer?");
        assert_eq!(a.options, vec!["the fast way", "the safe way"]);
        // a yes/no question after a list that is not the options stays yes/no
        let a = yn("I will change:\n- a.txt\n- b.txt\n\nShall I proceed?");
        assert!(a.yes_no);
        // not questions to click
        for t in ["The tests pass.", "Why does anyone write code like that?", "Isn't that great?", "What is your name?", "What would you like to do?", "How can I help?", "Here is the list:\n1. one\n2. two", "Would you like it? Anyway, here is the answer.", "", "Which?", "I wonder which is better?", "```\nShall I proceed?\n```"] {
            assert!(detect_question(t).is_none(), "wrongly detected: {t}: {:?}", detect_question(t));
        }
    }

    #[test]
    fn ask_user_arguments_make_exact_buttons() {
        let a = ask_from_args(&serde_json::json!({"question": "Which registry?", "options": ["Docker Hub", " GHCR ", "ghcr", ""]})).unwrap();
        assert_eq!((a.options.clone(), a.yes_no), (vec!["Docker Hub".to_string(), "GHCR".to_string()], false));
        assert_eq!(ask_text(&a), "Which registry?\n\n1. Docker Hub\n2. GHCR");
        let a = ask_from_args(&serde_json::json!({"question": "Proceed?"})).unwrap();
        assert!(a.yes_no && a.answers() == vec!["Yes", "No"] && ask_text(&a) == "Proceed?");
        assert!(ask_from_args(&serde_json::json!({"question": "Proceed?", "options": ["yes", "No"]})).unwrap().yes_no);
        assert!(ask_from_args(&serde_json::json!({"options": ["a"]})).is_none() && ask_from_args(&serde_json::json!({"question": "  "})).is_none());
        let many: Vec<String> = (0..20).map(|i| format!("o{i}")).collect();
        assert_eq!(ask_from_args(&serde_json::json!({"question": "q", "options": many})).unwrap().options.len(), 9);
    }
}
