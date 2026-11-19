//! The shrink step of the installer pages (F-INST-30): one small state machine that the window installer, the boot-time
//! install pages and any other front end draw, so the second deliberate step (the drive's name typed again and the ticked
//! box) is written once. It plans through [`crate::resize`] and changes nothing until `confirm` has both.
//!
//! Stages: `Pick` (the partitions that can be shrunk, and for the others the reason in plain words), `Plan` (the plan text
//! the person reads), `Done` or `Failed`. Going back from `Plan` to `Pick` is always possible and changes nothing.

use crate::planner::DiskModel;
use crate::resize::{plan_shrink, FsTool, Located, ShrinkPlan, ShrinkReport, ShrinkRequest};

const MIB: u64 = 1024 * 1024;

/// One row of the pick list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub part: u32,
    pub name: String,
    pub fs: String,
    pub bytes: u64,
    /// `None`: it can be shrunk; otherwise the refusal in plain words.
    pub refused: Option<String>,
}

/// Every partition of the disk, with the reason when it cannot be shrunk (checked with a one-MiB request, so only the
/// rules that do not depend on the size speak). Extended containers are listed with their reason too.
pub fn candidates(d: &DiskModel) -> Vec<Candidate> {
    d.parts
        .iter()
        .map(|p| {
            let bytes = p.bytes(d.sector);
            let refused = if bytes <= 2 * MIB {
                Some(format!("{} is too small to shrink", p.name))
            } else {
                // a minimum of one MiB stands in for the file system's own answer, which is asked when the person picks
                plan_shrink(d, &ShrinkRequest { part: p.index, new_bytes: bytes - MIB }, Some(MIB)).err()
            };
            Candidate { part: p.index, name: p.name.clone(), fs: p.fs.clone(), bytes, refused }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Pick,
    Plan(Box<ShrinkPlan>),
    Done(Vec<String>, u64),
    Failed(String),
}

pub struct ShrinkFlow {
    located: Located,
    tool: Box<dyn FsTool>,
    pub stage: Stage,
}

impl ShrinkFlow {
    pub fn new(located: Located, tool: Box<dyn FsTool>) -> ShrinkFlow {
        ShrinkFlow { located, tool, stage: Stage::Pick }
    }

    pub fn candidates(&self) -> Vec<Candidate> {
        candidates(&self.located.model)
    }

    /// The person picked a partition and a new size in MiB: make the plan (reads only). A refusal stays on the pick page.
    pub fn choose(&mut self, part: u32, to_mib: u64) -> Result<&ShrinkPlan, String> {
        let plan = self.located.plan(part, to_mib, self.tool.as_ref())?;
        self.stage = Stage::Plan(Box::new(plan));
        match &self.stage {
            Stage::Plan(p) => Ok(p),
            _ => unreachable!(),
        }
    }

    /// The plan text to show, if there is a plan.
    pub fn plan_text(&self) -> Option<String> {
        match &self.stage {
            Stage::Plan(p) => Some(p.text()),
            _ => None,
        }
    }

    pub fn back(&mut self) {
        if matches!(self.stage, Stage::Plan(_)) {
            self.stage = Stage::Pick;
        }
    }

    /// The second step: the drive's name typed again and the box ticked. Anything else changes nothing and the plan stays.
    pub fn confirm(&mut self, typed_name: &str, ack: bool) -> Result<(), String> {
        let Stage::Plan(plan) = &self.stage else { return Err("there is no plan to carry out".into()) };
        if !ack {
            return Err("tick the box to say you understand the file system is changed; nothing was changed".into());
        }
        if typed_name.trim() != plan.disk_name {
            return Err(format!("type the drive's name ({}) exactly as shown; nothing was changed", plan.disk_name));
        }
        let plan = plan.clone();
        match self.located.apply(&plan, typed_name.trim(), self.tool.as_ref()) {
            Ok(ShrinkReport { freed_bytes, notes }) => {
                self.stage = Stage::Done(notes, freed_bytes);
                Ok(())
            }
            Err(e) => {
                self.stage = Stage::Failed(e.clone());
                Err(e)
            }
        }
    }
}
