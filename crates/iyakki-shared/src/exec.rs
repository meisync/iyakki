//! Types moved out of `iyakki_agent::exec` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// Told about output as it arrives (the CLI tile shows a long run while it is still going).
pub type Sink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// What will be run, as the person reads it in the approval prompt.
pub fn command_line(program: &str, args: &[String]) -> String {
    let q = |a: &String| if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "\"'\\$`".contains(c)) { format!("'{}'", a.replace('\'', "'\\''")) } else { a.clone() };
    std::iter::once(program.to_string()).chain(args.iter().map(q)).collect::<Vec<_>>().join(" ")
}


/// Splits a line the way a shell would for quotes only (single, double, backslash); nothing is expanded or run.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let (mut out, mut cur, mut inword) = (Vec::new(), String::new(), false);
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\'' => {
                inword = true;
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("a quote is not closed".into()),
                    }
                }
            }
            '"' => {
                inword = true;
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => cur.push(it.next().ok_or("a quote is not closed")?),
                        Some(x) => cur.push(x),
                        None => return Err("a quote is not closed".into()),
                    }
                }
            }
            '\\' => {
                inword = true;
                cur.push(it.next().unwrap_or('\\'));
            }
            c if c.is_whitespace() => {
                if inword {
                    out.push(std::mem::take(&mut cur));
                    inword = false;
                }
            }
            c => {
                inword = true;
                cur.push(c);
            }
        }
    }
    if inword {
        out.push(cur);
    }
    Ok(out)
}
