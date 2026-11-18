//! The router's door on the fake machine: a record of what was asked, and a switch that makes it refuse.

use std::sync::Mutex;

use iyakki_core_api::router::Router;

#[derive(Default)]
pub struct FakeRouter {
    calls: Mutex<Vec<String>>,
    refuse: Mutex<bool>,
}

impl FakeRouter {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// Make the machine refuse runs and strategy changes.
    pub fn refuse(&self, on: bool) {
        *self.refuse.lock().unwrap() = on;
    }
}

impl Router for FakeRouter {
    fn begin_run(&self, label: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("begin_run({label})"));
        if *self.refuse.lock().unwrap() { Err("refused".into()) } else { Ok(()) }
    }
    fn allow_strategy(&self, name: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("allow_strategy({name})"));
        if *self.refuse.lock().unwrap() { Err("refused".into()) } else { Ok(()) }
    }
    fn record(&self, what: &str, detail: &str) {
        self.calls.lock().unwrap().push(format!("record({what} {detail})"));
    }
}
