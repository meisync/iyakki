//! The fleet's door on the fake machine: a record of what was asked, and a switch that makes it refuse.

use std::sync::Mutex;

use iyakki_core_api::fleet::Fleet;

#[derive(Default)]
pub struct FakeFleet {
    calls: Mutex<Vec<String>>,
    refuse: Mutex<bool>,
}

impl FakeFleet {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// Make the machine refuse every request.
    pub fn refuse(&self, on: bool) {
        *self.refuse.lock().unwrap() = on;
    }
    fn ask(&self, what: &str, label: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("{what}({label})"));
        if *self.refuse.lock().unwrap() { Err("refused".into()) } else { Ok(()) }
    }
}

impl Fleet for FakeFleet {
    fn read(&self, label: &str) -> Result<(), String> {
        self.ask("read", label)
    }
    fn join(&self, label: &str) -> Result<(), String> {
        self.ask("join", label)
    }
    fn manage(&self, label: &str) -> Result<(), String> {
        self.ask("manage", label)
    }
    fn assign(&self, label: &str) -> Result<(), String> {
        self.ask("assign", label)
    }
    fn receive(&self, label: &str) -> Result<(), String> {
        self.ask("receive", label)
    }
    fn record(&self, what: &str, detail: &str) {
        self.calls.lock().unwrap().push(format!("record({what} {detail})"));
    }
}
