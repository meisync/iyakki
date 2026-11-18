//! The router's models: the providers the person has configured, reached through the existing provider code (`ProviderClient`), so no
//! new way to talk to a model exists. An executor and the planner both use plain chat with NO tools offered; the planner's plan and an
//! executor's answer are small JSON texts the router reads itself.

use std::sync::Arc;

use crate::chat::ChatBackend;
use crate::providers::{ProviderChat, ProviderClient};
use crate::router::ModelPool;

/// `provider/model` ids, resolved to a client of that provider.
pub struct ProviderPool {
    make: Arc<dyn Fn(&str) -> Result<ProviderClient, String> + Send + Sync>,
}

impl ProviderPool {
    pub fn new(make: Arc<dyn Fn(&str) -> Result<ProviderClient, String> + Send + Sync>) -> ProviderPool {
        ProviderPool { make }
    }

    /// The providers of the settings, with their keys resolved at the moment of each request, as everywhere else.
    pub fn for_services(svc: Arc<crate::services::Services>) -> ProviderPool {
        ProviderPool::new(Arc::new(move |provider| svc.client_for(provider, false)))
    }
}

impl ModelPool for ProviderPool {
    fn backend(&self, id: &str) -> Option<Arc<dyn ChatBackend>> {
        // a provider id has no `/`, a model name may
        let (provider, model) = id.split_once('/')?;
        let client = (self.make)(provider).ok()?;
        Some(Arc::new(ProviderChat { client, model: model.to_string(), streamer: None }))
    }
}
