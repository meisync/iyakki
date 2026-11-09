//! SSH, SFTP and FTP protocol code with no account system in it.
//!
//! * Server side: [`sftp_proto`] (packet codec), [`sftp_fs`] (the `Jail`: a scope-confined file layer driven by two `Rule` closures),
//!   [`sftp_session`] (one SFTP conversation), [`ssh_server`] (the SSH transport; login and audit go through the [`ssh_server::Gate`] trait).
//! * Client side: [`rf_sftp`], [`rf_ssh`], [`rf_ftp`] (codecs and connections for the remote files client; they implement [`remotefs::RemoteFs`]).
//! * [`sshkey`]: ed25519 keys and the OpenSSH key file format. [`b64`] and [`remotefs`] are the small helpers the others share.
//!
//! `iyakki-agent` re-exports every module under its old path (`iyakki_agent::ssh_server` ...) and keeps the glue: the account policy,
//! vault, firewall rules and the connection manager.

pub mod b64;
pub mod remotefs;
pub mod rf_ftp;
pub mod rf_sftp;
pub mod rf_ssh;
pub mod sftp_fs;
pub mod sftp_proto;
pub mod sftp_session;
pub mod ssh_server;
pub mod sshkey;

#[cfg(any(test, feature = "testing"))]
pub mod testdir;
#[cfg(any(test, feature = "testing"))]
pub mod testworld;
#[cfg(test)]
mod sftp_tests;
#[cfg(test)]
mod rf_tests;
