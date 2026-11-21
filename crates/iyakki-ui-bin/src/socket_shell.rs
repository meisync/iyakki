//! The whole shell over the core's UI socket (F-SPLIT-03, feature `socket-shell`): `iyakki-ui --socket PATH` runs the launcher and every app against
//! remote objects of the core. What is installed here is what `iyakki_compose::wire()` installs for the in-process program, built from the socket:
//!
//! * the services of the shell (`RemoteServicesHandle`), the accounts (`UsersSrc` over the served `AccountsApi`) and the audit log (`AuditApi` root),
//! * the process-wide agent facade (`RemoteAgentApi`: the traits the core serves are remote, the rest answer their quiet defaults),
//! * the core handle (`SocketCore`: settings, vault status, flows, the assistant, the cameras, the sound (mixer, outputs, cues, sound route) and the app statuses are served),
//! * the host of the shell (`RemoteHost`: the core's own, asked over the socket: name, network, power, sleep and WebAssembly runs).
//!
//! The in-process agent never comes in: this module and the crates it uses are checked by `tests/no_agent.rs` to contain neither the agent nor the
//! core. A part the core does not serve answers quietly (D-SPLIT-03-QUIET); nothing answers with a made-up success.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use iyakki_agent_api::ServicesHandle;
use iyakki_core_api::{Core, HostApi};
use iyakki_shell::shell::{RemoteParts, ShellEnv};
use iyakki_shell::wiring::CoreInputs;
use iyakki_ui_rpc::{RemoteAgentApi, RemoteServicesHandle};

use crate::{connect, NotReady};

/// The host of a UI program is the core's own, asked over the socket (`RemoteServicesHandle::host`): the machine's name, network status, power,
/// sleep and WebAssembly runs. A core that has no host to serve answers like a machine that cannot (`can_power` is false) and the power cards say so;
/// nothing here switches the machine or runs a program by itself.
fn host_of(services: &RemoteServicesHandle) -> Arc<dyn HostApi> {
    services.host()
}

/// The file scope of the core (what a person's files, pickers and tools may reach) asked of the core: every question is a call of the services over the
/// socket, so the program never reads the machine's mounts or home folders to decide (the rules, the symlink handling and the audit are the core's).
pub struct ServicesFileScope(pub Arc<dyn ServicesHandle>);

impl iyakki_core_api::FileScope for ServicesFileScope {
    fn check_read(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &Path) -> Result<(), String> {
        self.0.scope_check_read(policy, user, p)
    }
    fn check_write(&self, policy: &iyakki_sec::policy::Policy, user: &str, p: &Path) -> Result<(), String> {
        self.0.scope_check_write(policy, user, p)
    }
    fn home_root(&self, user: &str, data_dir: &Path) -> std::path::PathBuf {
        self.0.scope_home_root(user, data_dir)
    }
    fn home_root_for_writes(&self, user: &str, data_dir: &Path) -> std::path::PathBuf {
        self.0.scope_home_root_for_writes(user, data_dir)
    }
    fn system_home(&self, user: &str) -> Option<std::path::PathBuf> {
        self.0.scope_system_home(user)
    }
    fn roots(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &Path) -> iyakki_core_api::FileRoots {
        self.0.scope_roots(policy, user, data_dir)
    }
    fn roots_with_home(&self, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &Path, home: std::path::PathBuf) -> iyakki_core_api::FileRoots {
        self.0.scope_roots_with_home(policy, user, data_dir, home)
    }
    fn resolve(&self, cwd: &Path, input: &str) -> Result<std::path::PathBuf, String> {
        self.0.scope_resolve(cwd, input)
    }
    fn resolve_for_write(&self, cwd: &Path, input: &str) -> Result<std::path::PathBuf, String> {
        self.0.scope_resolve_for_write(cwd, input)
    }
    fn ensure_parent(&self, path: &Path) -> Result<(), String> {
        self.0.scope_ensure_parent(path)
    }
}

static CORE: OnceLock<Arc<dyn Core>> = OnceLock::new();
static SERVICES: OnceLock<Arc<dyn ServicesHandle>> = OnceLock::new();

/// The `CoreFactory` of the socket program: the shell gets the one core handle of this connection.
fn core_of_the_socket(_env: &ShellEnv, _inputs: CoreInputs) -> Arc<dyn Core> {
    CORE.get().expect("the core handle is installed before the first shell").clone()
}

fn services_of_the_socket(_dir: &Path, _quick: Arc<dyn iyakki_proxy::Upstream>, _slow: Arc<dyn iyakki_proxy::Upstream>) -> Result<Arc<dyn ServicesHandle>, String> {
    SERVICES.get().cloned().ok_or_else(|| "no services of the socket installed".to_string())
}

/// Why the program cannot show the screen: said in one sentence; `main` turns each into its exit status.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The core does not serve the object calls (the `rpc` switch of the socket is off, or an older core).
    NoObjectCalls,
    /// The core did not answer, or answered something the shell cannot start on.
    Core(String),
}

/// Connect, install what the shell is given and build its environment. `tries` connection attempts, `pause` between them (the core opens its socket
/// as it starts: a program the core starts first waits for it instead of giving up).
pub fn environment(path: &Path, hwcheck: bool, tries: u32, pause: &dyn Fn()) -> Result<ShellEnv, Refused> {
    let connected = {
        let mut last = String::new();
        let mut got = None;
        for n in 0..tries.max(1) {
            match connect(path) {
                Ok(Ok(c)) => {
                    got = Some(c);
                    break;
                }
                Ok(Err(NotReady::NoObjectCalls)) => return Err(Refused::NoObjectCalls),
                Err(e) => {
                    last = e;
                    if n + 1 < tries {
                        pause();
                    }
                }
            }
        }
        got.ok_or(Refused::Core(last))?
    };
    let machine = connected.info.machine.clone();
    let services = Arc::new(connected.services);
    install(path, &services).map_err(Refused::Core)?;
    let handle: Arc<dyn ServicesHandle> = services.clone();
    let users = iyakki_accounts::UsersRef::new(Arc::new(iyakki_agent_api::UsersSrc(services.accounts())));
    let audit = services.audit();
    ShellEnv::from_remote(RemoteParts { services: handle, audit, users, host: host_of(&services), ai_conf: "/etc/iyakki/ai.conf".into(), hostname: machine, hwcheck }).map_err(|e| Refused::Core(format!("{e:?}")))
}

/// Put the socket's handles where the shell and the apps look for them (each is a one-shot cell: the first program start wins).
fn install(path: &Path, services: &Arc<RemoteServicesHandle>) -> Result<(), String> {
    let rpc = services.rpc().clone();
    let conn = iyakki_ui_client::client::Connection::unix(path, Duration::from_secs(5)).map_err(|e| format!("cannot reach the core at {}: {}", path.display(), e.message))?;
    let svc: Arc<dyn ServicesHandle> = services.clone();
    let scope: Arc<dyn iyakki_core_api::FileScope> = Arc::new(ServicesFileScope(svc.clone()));
    let sound = services.core_sound();
    let facets = iyakki_ui_client::socketcore::SoundFacets { mixer: sound.clone(), sound: sound.clone(), cues: sound.clone(), route: sound.clone() };
    let _ = CORE.set(Arc::new(iyakki_ui_client::socketcore::SocketCore::new(conn).with_file_scope(scope.clone()).with_sound(facets).with_statuses(sound)));
    let _ = iyakki_apps::install_file_scope(scope);
    // `iyakki dev state` is answered by the core: the program tells it it is drawing and hands the raw frame over
    let _ = services.install_dev_state();
    let _ = SERVICES.set(svc);
    let _ = iyakki_apps::install_agent(Arc::new(RemoteAgentApi::new(rpc)));
    // the assistant's runs are the core's: the chat sends a request as data and reads the events back (`apps::core_run::remote_run`)
    let _ = iyakki_apps::install_chat_runs(services.chat_runs());
    let _ = iyakki_shell::wiring::install_core_factory(core_of_the_socket);
    let _ = iyakki_shell::wiring::install_services_factory(services_of_the_socket);
    Ok(())
}
