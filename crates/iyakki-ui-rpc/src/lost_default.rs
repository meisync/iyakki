// Types whose quiet value of a lost call is `Default` (written by dist/split/serde_sweep.py; add by hand below the marker).
lost_default!(
    iyakki_shared::domains::ListenerCfg,
    iyakki_shared::domains::Status,
    iyakki_shared::lab::Catalogue,
    iyakki_shared::pending::Settings,
    iyakki_shared::platform_types::RemovePlan,
    iyakki_shared::quickinstall::Facts,
    iyakki_shared::revise::Proposal,
    iyakki_shared::router::RouterConfig,
    iyakki_shared::sftp::Config,
    iyakki_shared::sftp::Status,
    iyakki_shared::sshtunnel_mgr::TestReport,
    iyakki_shared::store_catalog::Config,
    iyakki_shared::store_core::Refresh,
    iyakki_shared::store_lifecycle::Report,
    iyakki_shared::store_private::StatusInfo,
    iyakki_shared::store_revocation::Revocations,
    iyakki_shared::store_trust::Roots,
    iyakki_shared::store_updatecheck::Report,
    iyakki_shared::vpn_mgr::Status,
    // the FLOWS jobs (F-SPLIT-03, D-S5-01): a lost call shows an empty book, no counts, the default policy
    iyakki_flow::engine::Counts,
    iyakki_flow::store::FlowMeta,
    iyakki_flow::assist_store::Global,
    iyakki_flow::assist_store::Book,
    iyakki_shared::flows_channel_policy::Policy
);
