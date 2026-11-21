//! The fake [`Drivers`]: a canned machine, canned rows and installed packs, every call recorded, nothing read from this computer.

use std::sync::Mutex;

use iyakki_core_api::drivers::{Drivers, Machine, Record, Row};

#[derive(Default)]
struct State {
    machine: Machine,
    rows: Vec<Row>,
    installed: Vec<Record>,
    files: Vec<(String, (usize, usize))>,
    disabled: Vec<String>,
    calls: Vec<String>,
}

#[derive(Default)]
pub struct FakeDrivers {
    state: Mutex<State>,
}

impl FakeDrivers {
    pub fn new() -> FakeDrivers {
        FakeDrivers::default()
    }
    pub fn set_machine(&self, m: Machine) {
        self.state.lock().unwrap().machine = m;
    }
    pub fn set_rows(&self, r: Vec<Row>) {
        self.state.lock().unwrap().rows = r;
    }
    pub fn set_installed(&self, r: Vec<Record>) {
        self.state.lock().unwrap().installed = r;
    }
    /// The files the record of pack `id` lists and how many are present.
    pub fn set_pack_files(&self, id: &str, listed: usize, present: usize) {
        self.state.lock().unwrap().files.push((id.to_string(), (listed, present)));
    }
    pub fn set_disabled(&self, id: &str) {
        self.state.lock().unwrap().disabled.push(id.to_string());
    }
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
    fn rec(&self, what: &str) {
        self.state.lock().unwrap().calls.push(what.to_string());
    }
}

impl Drivers for FakeDrivers {
    fn machine(&self) -> Machine {
        self.rec("machine");
        self.state.lock().unwrap().machine.clone()
    }
    fn rows(&self) -> Vec<Row> {
        self.rec("rows");
        self.state.lock().unwrap().rows.clone()
    }
    fn installed(&self) -> Vec<Record> {
        self.rec("installed");
        self.state.lock().unwrap().installed.clone()
    }
    fn is_disabled(&self, id: &str) -> bool {
        self.rec("is_disabled");
        self.state.lock().unwrap().disabled.iter().any(|d| d == id)
    }
    fn pack_files(&self, id: &str) -> Option<(usize, usize)> {
        self.rec("pack_files");
        self.state.lock().unwrap().files.iter().find(|f| f.0 == id).map(|f| f.1)
    }
    fn remove_pack(&self, id: &str) -> Result<String, String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("remove_pack({id})"));
        let n = s.files.len();
        s.files.retain(|f| f.0 != id);
        if s.files.len() == n {
            return Err(format!("no driver pack `{id}` is installed"));
        }
        Ok(format!("removed {id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_answers_what_it_was_given_records_each_call_and_a_default_os_has_no_drivers() {
        let f = FakeDrivers::new();
        assert!(f.rows().is_empty() && f.installed().is_empty() && !f.is_disabled("x"));
        f.set_machine(Machine { vendor: "Acme".into(), ..Default::default() });
        f.set_disabled("pack-a");
        assert_eq!(f.machine().vendor, "Acme");
        assert!(f.is_disabled("pack-a") && !f.is_disabled("pack-b"));
        assert_eq!(f.calls(), ["rows", "installed", "is_disabled", "machine", "is_disabled", "is_disabled"]);
        use iyakki_core_api::Os;
        let os = crate::FakeOs::new();
        assert!(os.drivers().rows().is_empty());
    }

    #[test]
    fn the_fake_counts_the_files_of_a_pack_and_removes_it_once() {
        let f = FakeDrivers::new();
        assert_eq!(f.pack_files("p"), None);
        assert!(f.remove_pack("p").is_err());
        f.set_pack_files("p", 3, 2);
        assert_eq!(f.pack_files("p"), Some((3, 2)));
        assert_eq!(f.remove_pack("p").unwrap(), "removed p");
        assert_eq!(f.pack_files("p"), None);
        assert!(f.remove_pack("p").is_err());
    }
}
