//! The router's door on the fake machine: a record of what was asked, and a switch that makes it refuse.

use std::sync::Mutex;

use iyakki_core_api::router::{RouteModel, Router};

#[derive(Default)]
pub struct FakeRouter {
    calls: Mutex<Vec<String>>,
    refuse: Mutex<bool>,
    models: Mutex<Vec<RouteModel>>,
}

impl FakeRouter {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// The models this fake machine offers to the routing decision.
    pub fn set_models(&self, m: Vec<RouteModel>) {
        *self.models.lock().unwrap() = m;
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
    fn allow_listen(&self, label: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("allow_listen({label})"));
        if *self.refuse.lock().unwrap() { Err("refused".into()) } else { Ok(()) }
    }
    fn ask_step_tool(&self, label: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("ask_step_tool({label})"));
        if *self.refuse.lock().unwrap() { Err("refused".into()) } else { Ok(()) }
    }
    fn allow_strategy(&self, name: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("allow_strategy({name})"));
        if *self.refuse.lock().unwrap() { Err("refused".into()) } else { Ok(()) }
    }
    fn models(&self) -> Vec<RouteModel> {
        self.models.lock().unwrap().clone()
    }
    fn record(&self, what: &str, detail: &str) {
        self.calls.lock().unwrap().push(format!("record({what} {detail})"));
    }
}
