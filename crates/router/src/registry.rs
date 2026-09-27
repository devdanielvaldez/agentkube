use crate::{ModelRoutingProfile, RegistryError};
use agentkube_agents::{ModelName, ProviderName};
use agentkube_providers::ModelProvider;
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

/// Thread-safe registry of provider adapters and model routing profiles.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<BTreeMap<ProviderName, RegisteredProvider>>,
}

#[derive(Clone)]
pub(crate) struct RegisteredProvider {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) profiles: BTreeMap<ModelName, ModelRoutingProfile>,
}

impl ProviderRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            providers: RwLock::new(BTreeMap::new()),
        }
    }

    /// Registers one adapter with exactly one profile per advertised model.
    pub fn register(
        &self,
        provider: Arc<dyn ModelProvider>,
        profiles: impl IntoIterator<Item = (ModelName, ModelRoutingProfile)>,
    ) -> Result<(), RegistryError> {
        let name = provider.name().clone();
        let mut profile_map = BTreeMap::new();
        for (model, profile) in profiles {
            if profile_map.insert(model.clone(), profile).is_some() {
                return Err(RegistryError::DuplicateProfile(model));
            }
        }
        for (model, _) in provider.capabilities().models() {
            if !profile_map.contains_key(model) {
                return Err(RegistryError::MissingProfile(model.clone()));
            }
        }
        for model in profile_map.keys() {
            if provider.capabilities().model(model).is_none() {
                return Err(RegistryError::UnknownModel(model.clone()));
            }
        }

        let mut providers = self
            .providers
            .write()
            .map_err(|_| RegistryError::Unavailable("registry lock is poisoned"))?;
        if providers.contains_key(&name) {
            return Err(RegistryError::DuplicateProvider(name));
        }
        providers.insert(
            name,
            RegisteredProvider {
                provider,
                profiles: profile_map,
            },
        );
        Ok(())
    }

    /// Removes a provider and returns whether it was registered.
    pub fn remove(&self, name: &ProviderName) -> Result<bool, RegistryError> {
        Ok(self
            .providers
            .write()
            .map_err(|_| RegistryError::Unavailable("registry lock is poisoned"))?
            .remove(name)
            .is_some())
    }

    /// Returns the number of registered provider adapters.
    pub fn len(&self) -> Result<usize, RegistryError> {
        Ok(self
            .providers
            .read()
            .map_err(|_| RegistryError::Unavailable("registry lock is poisoned"))?
            .len())
    }

    /// Returns whether the registry has no providers.
    pub fn is_empty(&self) -> Result<bool, RegistryError> {
        self.len().map(|length| length == 0)
    }

    pub(crate) fn snapshot(
        &self,
    ) -> Result<BTreeMap<ProviderName, RegisteredProvider>, RegistryError> {
        Ok(self
            .providers
            .read()
            .map_err(|_| RegistryError::Unavailable("registry lock is poisoned"))?
            .clone())
    }
}
