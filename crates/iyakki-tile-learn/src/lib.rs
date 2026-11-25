//! The LEARN tile (alpha.8 E3): the study plans, quizzes and lessons the chat makes (`docs/STUDY.md`), as a crate that depends on the toolkit
//! (`iyakki-uikit`) and the pure model crate `iyakki-study-model` only. Everything it needs from the rest of the system is one trait,
//! [`port::LearnPort`] (with the lesson writer and the picture drawer as two small objects of their own); the shell adapter
//! (`crates/iyakki-apps/src/apps/learn.rs`) and the real port (`learn_port.rs`) are the only code that names the services.

pub mod app;
pub mod draw;
pub mod lesson;
pub mod port;

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod fake_tests;

pub use app::{Flow, LearnApp, Tab, View, ACCENT};
pub use port::{LearnPort, LessonWriter, Pictures, WriteEvent};
