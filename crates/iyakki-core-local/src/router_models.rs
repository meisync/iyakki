//! The model registry the device router answers its routing decision from (F-ROUTER-01).

static PUBLISHED_MODELS: std::sync::RwLock<Vec<iyakki_core_api::router::RouteModel>> = std::sync::RwLock::new(Vec::new());

/// The model registry of the running program (built by the agent from the provider settings): the list [`LocalRouter`] answers the routing
/// decision from. It replaces the earlier list whole. No key and no address is in a [`RouteModel`](iyakki_core_api::router::RouteModel).
pub fn publish_models(models: Vec<iyakki_core_api::router::RouteModel>) {
    *PUBLISHED_MODELS.write().unwrap_or_else(|e| e.into_inner()) = models;
}

pub(crate) fn published_models() -> Vec<iyakki_core_api::router::RouteModel> {
    PUBLISHED_MODELS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

