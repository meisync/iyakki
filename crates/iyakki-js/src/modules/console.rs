//! `console.log` and friends: collected for the browser's developer view, never printed anywhere else.

use std::any::Any;

use crate::engine::WebModule;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleLine {
    pub level: String,
    pub text: String,
}

#[derive(Default)]
pub struct ConsoleModule {
    pub lines: Vec<ConsoleLine>,
    /// Lines dropped because a page logged too much.
    pub dropped: usize,
}

const MAX_LINES: usize = 500;
const MAX_LINE: usize = 2000;

impl ConsoleModule {
    pub fn new() -> ConsoleModule {
        ConsoleModule::default()
    }
}

impl WebModule for ConsoleModule {
    fn name(&self) -> &'static str {
        "console"
    }

    fn prelude(&self) -> &'static str {
        r#"
(function (g) {
  'use strict';
  const fmt = (a) => {
    if (typeof a === 'string') return a;
    try {
      if (a instanceof Error) return String(a) + (a.stack ? '\n' + a.stack : '');
      if (typeof a === 'object' && a !== null) return JSON.stringify(a);
    } catch (e) {}
    return String(a);
  };
  const out = (level) => (...a) => { __host('console', 'log', JSON.stringify({ level, text: a.map(fmt).join(' ') })); };
  g.console = {
    log: out('log'), info: out('info'), warn: out('warn'), error: out('error'), debug: out('debug'), trace: out('trace'),
    dir: out('log'), table: out('log'),
    assert(c, ...a) { if (!c) out('error')('Assertion failed:', ...a); },
    group() {}, groupEnd() {}, time() {}, timeEnd() {}, clear() {}, count() {},
  };
})(globalThis);
"#
    }

    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        match op {
            "log" => {
                if self.lines.len() >= MAX_LINES {
                    self.dropped += 1;
                } else {
                    let mut text = args["text"].as_str().unwrap_or("").to_string();
                    if text.len() > MAX_LINE {
                        let mut cut = MAX_LINE;
                        while !text.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        text.truncate(cut);
                        text.push('…');
                    }
                    self.lines.push(ConsoleLine { level: args["level"].as_str().unwrap_or("log").to_string(), text });
                }
                Ok(serde_json::Value::Null)
            }
            other => Err(format!("console has no operation {other}")),
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JsEngine, Limits};

    #[test]
    fn console_lines_are_collected_with_their_level() {
        let mut e = JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new())]).unwrap();
        e.eval("t", "console.log('a', 1, {x: [2]}); console.error(new Error('boom').message); console.warn(null, undefined); console.assert(1 === 2, 'nope')").unwrap();
        let c = e.module::<ConsoleModule>("console").unwrap();
        let got: Vec<(&str, &str)> = c.lines.iter().map(|l| (l.level.as_str(), l.text.as_str())).collect();
        assert_eq!(got, vec![("log", "a 1 {\"x\":[2]}"), ("error", "boom"), ("warn", "null undefined"), ("error", "Assertion failed: nope")]);
    }

    #[test]
    fn a_page_that_logs_too_much_is_cut_off() {
        let mut e = JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new())]).unwrap();
        e.eval("t", "for (let i = 0; i < 600; i++) console.log('x'.repeat(5000))").unwrap();
        let c = e.module::<ConsoleModule>("console").unwrap();
        assert_eq!(c.lines.len(), 500);
        assert_eq!(c.dropped, 100);
        assert!(c.lines[0].text.chars().count() <= 2001, "long lines are shortened");
    }
}
