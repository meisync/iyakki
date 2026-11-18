//! A program on a pseudo-terminal (Linux): [`crate::terminal::Terminal`] over the pty.

use crate::pty::{Pty, Read};
use crate::terminal::{Link, LinkState, Terminal};

pub struct PtyLink(Pty);

impl PtyLink {
    /// Start `program` on a new terminal of `cols` x `rows`; the child gets exactly the environment given.
    pub fn spawn(program: &str, args: &[String], env: &[(String, String)], cols: usize, rows: usize) -> std::io::Result<PtyLink> {
        Ok(PtyLink(Pty::spawn(program, args, env, cols.clamp(1, 500) as u16, rows.clamp(1, 200) as u16)?))
    }
}

impl Link for PtyLink {
    fn read(&mut self, out: &mut Vec<u8>) -> LinkState {
        match self.0.read_available(out) {
            Ok(Read::Data) => LinkState::Open,
            Ok(Read::Closed) | Err(_) => LinkState::Closed,
        }
    }
    fn write(&mut self, bytes: &[u8]) {
        let _ = self.0.write_all(bytes);
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.0.resize(cols, rows);
    }
    fn exit_code(&mut self) -> Option<i32> {
        self.0.exit_code()
    }
    fn close(&mut self) {
        self.0.close();
    }
}

pub type Session = Terminal<PtyLink>;

impl Terminal<PtyLink> {
    pub fn start(program: &str, args: &[String], env: &[(String, String)], cols: usize, rows: usize) -> std::io::Result<Session> {
        Ok(Terminal::new(PtyLink::spawn(program, args, env, cols, rows)?, cols, rows))
    }
}
