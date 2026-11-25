//! The STORE tile's side of the store: the trait the screens call (`engine`) and its fake (`fake`).

pub mod ai;
pub mod drivers;
pub mod engine;
pub mod facts;
pub mod setup_hw;
pub mod shell;
pub mod switch;
pub mod setup_svc;
pub mod fake;

#[cfg(test)]
mod fake_tests;
