//! The git keys of one account over the UI socket (F-SPLIT-03 part 3, D-SPLIT-03-SERVED): `GitKeys` is a concrete type over a folder and a secret
//! store, so the UI builds its own over the core's folder and a store that crosses the socket for writing and listing only. A private key, a
//! passphrase or a token is never answered to the UI (D-SPLIT-03-SECRETS): `get` is refused on both ends, a key is generated, imported,
//! listed (from the table the core keeps), deleted and unlocked, and everything that needs the key itself (signing, a push, a clone) runs in the
//! core's own git kit.

use std::sync::Arc;

use iyakki_shared::gitkeys::{AuditFn, GitKeys, SecretStore, Session};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::served::{Served, NOT_SERVED};
use crate::{de, ser, Lost, Obj, Registry, RemoteServicesHandle, Rpc, ServicesRoot};

/// What a git key store may touch: the names the git keys use, nothing else of the person's vault.
fn git_name(n: &str) -> bool {
    (n.starts_with("ssh.") || n.starts_with("git.")) && n.len() <= 120 && iyakki_shared::vault::valid_secret_name(n)
}

pub(crate) fn serve_git(root: &ServicesRoot, reg: &Registry<'_>, method: &str, a: Value) -> Option<Result<Value, String>> {
    Some(match method {
        "git_keys_dir" => (|| {
            let (user,): (String,) = de(a)?;
            let quiet: AuditFn = Arc::new(|_, _| {});
            let keys = root.svc.git_keys(&user, quiet, Arc::new(Session { pass: Default::default(), approved: Default::default() }));
            ser(&keys.dir().to_path_buf())
        })(),
        "git_store" => (|| {
            let (user,): (String,) = de(a)?;
            let make = crate::served::ServedParts::need(&root.parts.secret_store, "secret store")?;
            ser(&reg.add(Obj::Served(Arc::new(GitStoreObj { store: make(&user), audit: root.audit.clone() }))))
        })(),
        _ => return None,
    })
}

/// The names of the git keys of one account in the core's vault, writable and listable, never readable.
struct GitStoreObj {
    store: Arc<dyn SecretStore>,
    audit: AuditFn,
}
impl Served for GitStoreObj {
    fn iface(&self) -> &'static str {
        "GitSecretStore"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "names" => ser(&self.store.names().map(|v| v.into_iter().filter(|n| git_name(n)).collect::<Vec<String>>())),
            "set" => {
                let (name, value): (String, String) = de(a)?;
                if !git_name(&name) {
                    return Err(format!("{NOT_SERVED}: `{name}` is not a name the git keys use"));
                }
                (self.audit)("git.store.set", &name);
                ser(&self.store.set(&name, &value))
            }
            "delete" => {
                let (name,): (String,) = de(a)?;
                if !git_name(&name) {
                    return Err(format!("{NOT_SERVED}: `{name}` is not a name the git keys use"));
                }
                (self.audit)("git.store.delete", &name);
                ser(&self.store.delete(&name))
            }
            "get" => Err(format!("{NOT_SERVED}: a key, a passphrase or a token is read in the core, never handed to the UI")),
            other => Err(format!("GitSecretStore has no call {other}")),
        }
    }
}

impl RemoteServicesHandle {
    /// The git keys of an account: the folder is the core's, the store crosses the socket for names and writes only.
    pub(crate) fn m_git_keys(&self, user: &str, audit: AuditFn, session: Arc<Session>) -> GitKeys {
        let dir = self.rpc.call::<std::path::PathBuf>("ServicesHandle", 0, "git_keys_dir", json!([user]));
        let obj = self.rpc.call::<u64>("ServicesHandle", 0, "git_store", json!([user]));
        match (dir, obj) {
            (Ok(dir), Ok(obj)) => GitKeys::new(&dir, Arc::new(RemoteGitStore { rpc: self.rpc.clone(), obj }), audit, session),
            (Err(e), _) | (_, Err(e)) => <GitKeys as Lost>::lost(&e),
        }
    }
}

struct RemoteGitStore {
    rpc: Rpc,
    obj: u64,
}
impl Drop for RemoteGitStore {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}
impl SecretStore for RemoteGitStore {
    fn get(&self, _name: &str) -> Result<Zeroizing<String>, String> {
        Err(format!("{NOT_SERVED}: a key, a passphrase or a token is read in the core, never handed to the UI"))
    }
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        crate::served::call(&self.rpc, "GitSecretStore", self.obj, "set", json!([name, value]))
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        crate::served::call(&self.rpc, "GitSecretStore", self.obj, "delete", json!([name]))
    }
    fn names(&self) -> Result<Vec<String>, String> {
        crate::served::call(&self.rpc, "GitSecretStore", self.obj, "names", json!([]))
    }
}
