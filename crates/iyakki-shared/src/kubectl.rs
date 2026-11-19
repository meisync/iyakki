//! `kubectl` against the managed cluster as a trait (moved from iyakki-dash, which re-exports it), so that code in shared can use it.

/// `kubectl` against the managed cluster; `Err` is a plain sentence.
pub trait Kubectl: Send + Sync {
    fn kubectl(&self, args: &[&str]) -> Result<String, String>;
    /// Like [`Kubectl::kubectl`] for answers that can be large (every pod of the cluster).
    fn kubectl_big(&self, args: &[&str]) -> Result<String, String> {
        self.kubectl(args)
    }
}

