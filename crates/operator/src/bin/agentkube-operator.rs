//! `agentkube-operator`: durable API with reconcile and dispatch loops.
//!
//! One process serves the versioned HTTP API on SQLite storage and runs the
//! control loops: crash recovery once at boot, agent-status reconciliation on
//! a tick, scheduled dispatch with embedded model execution, and worker
//! heartbeats. Boot failures exit non-zero; runtime tick failures are loud
//! but keep serving the API.

use agentkube_agents::{ModelName, ProviderName};
use agentkube_api::{ApiState, NodeInfo, router, serve};
use agentkube_config::{AgentKubeConfig, ConfigLoader, EnvironmentSource};
use agentkube_core::NodeId;
use agentkube_operator::{
    Dispatcher, ProviderDescriptor, build_catalogs, discover_ollama_models, reconcile_once, recover,
};
use agentkube_providers::ModelCapabilities;
use agentkube_providers_http::{OllamaProvider, OpenAiProvider};
use agentkube_queue::{InMemoryTaskQueue, TaskQueue};
use agentkube_router::{
    DataResidency, ModelRouter, ModelRouting, ModelRoutingProfile, ProviderRegistry,
};
use agentkube_scheduler::{NodeLocality, Scheduler};
use agentkube_sqlite::SqliteStores;
use agentkube_workers::{RepositoryWorkerStateStore, SingleTurnRuntime};
use std::{
    error::Error,
    io,
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = ConfigLoader::new()
        .with_source(EnvironmentSource::from_current_process("AGENTKUBE"))
        .load()?;
    let maximum_body_size = usize::try_from(config.control_plane().max_request_body_bytes())
        .map_err(|_| io::Error::other("request body limit exceeds platform usize"))?;

    let stores = SqliteStores::open(config.storage().data_dir().join("agentkube.db"))?;
    let tasks = stores.tasks();
    let agents = stores.agents();
    let deployments = stores.deployments();
    let queue: Arc<dyn TaskQueue> = Arc::new(InMemoryTaskQueue::new());

    recover(&tasks, Arc::clone(&queue)).await?;

    let mut state = ApiState::new(
        Arc::clone(&agents),
        Arc::clone(&deployments),
        Arc::clone(&tasks),
        Arc::clone(&queue),
    );
    if let Some(token) = config.auth().token() {
        state = state.with_auth_token(token.expose());
    }

    let worker_state = RepositoryWorkerStateStore::new(Arc::clone(&tasks));
    worker_state
        .resync()
        .await
        .map_err(|error| io::Error::other(format!("worker state resync failed: {error}")))?;

    let (registry, providers) = register_providers(&config).await?;
    let provider_total = providers.len();
    let runtime = Arc::new(SingleTurnRuntime::new(
        Arc::new(ModelRouter::new(registry)) as Arc<dyn ModelRouting>
    ));
    let node_id = NodeId::new();
    let active = Arc::new(AtomicU16::new(0));
    let dispatcher = Dispatcher::new(
        Arc::clone(&tasks),
        Arc::clone(&agents),
        Arc::clone(&queue),
        worker_state.clone(),
        runtime,
        Scheduler::default(),
        node_id,
        providers,
        config.worker().concurrency().get(),
        config.worker().lease_timeout(),
        state.node_registry(),
        Arc::clone(&active),
    );

    let listener = TcpListener::bind(config.control_plane().bind_address()).await?;
    println!(
        "AgentKube operator {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr()?
    );
    println!(
        "Reconcile every {}, dispatch every {}, {} provider(s) registered",
        config.operator().reconcile_interval(),
        config.operator().dispatch_interval(),
        provider_total,
    );
    let server = tokio::spawn(serve(listener, router(state.clone(), maximum_body_size)));

    let mut reconcile_tick = tokio::time::interval(config.operator().reconcile_interval().get());
    let mut dispatch_tick = tokio::time::interval(config.operator().dispatch_interval().get());
    let mut heartbeat_tick = tokio::time::interval(config.worker().heartbeat_interval().get());
    let mut last_blocked = Vec::new();
    let mut last_unmanaged = Vec::new();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("AgentKube operator shutting down");
                break;
            }
            _ = reconcile_tick.tick() => {
                match reconcile_once(&agents, &deployments, &worker_state).await {
                    Ok(summary) => {
                        for name in &summary.updated {
                            println!("reconcile: persisted status for agent {name:?}");
                        }
                        if summary.blocked != last_blocked {
                            last_blocked.clone_from(&summary.blocked);
                            if !summary.blocked.is_empty() {
                                eprintln!("reconcile: blocked agents: {:?}", summary.blocked);
                            }
                        }
                        if summary.unmanaged != last_unmanaged {
                            last_unmanaged.clone_from(&summary.unmanaged);
                            if !summary.unmanaged.is_empty() {
                                eprintln!("reconcile: unmanaged deployments (instance management not yet wired): {:?}", summary.unmanaged);
                            }
                        }
                    }
                    Err(error) => eprintln!("reconcile cycle failed: {error}"),
                }
            }
            _ = dispatch_tick.tick() => {
                if let Err(error) = dispatcher.dispatch_once().await {
                    eprintln!("dispatch cycle failed: {error}");
                }
            }
            _ = heartbeat_tick.tick() => {
                state.node_registry().record_heartbeat(NodeInfo::new(
                    node_id,
                    active.load(Ordering::SeqCst),
                    config.worker().concurrency(),
                )).await;
            }
        }
    }
    server.abort();
    Ok(())
}

async fn register_providers(
    config: &AgentKubeConfig,
) -> Result<(Arc<ProviderRegistry>, Vec<ProviderDescriptor>), Box<dyn Error>> {
    let (discovered, discovery_warning) = if config.providers().ollama_enabled() {
        discover_ollama_models(config.providers().ollama_base_url()).await
    } else {
        (Vec::new(), None)
    };
    if let Some(warning) = discovery_warning {
        eprintln!("providers: {warning}");
    }
    let (catalogs, warnings) = build_catalogs(config.providers(), &discovered)?;
    for warning in warnings {
        eprintln!("providers: {warning}");
    }
    let registry = Arc::new(ProviderRegistry::new());
    let mut providers = Vec::new();
    if !catalogs.ollama.is_empty() {
        let adapter: Arc<dyn agentkube_providers::ModelProvider> = Arc::new(OllamaProvider::new(
            config.providers().ollama_base_url(),
            catalogs.ollama.clone(),
        )?);
        registry.register(adapter, profiles(&catalogs.ollama, DataResidency::Local)?)?;
        providers.push(ProviderDescriptor::new(
            ProviderName::new(OllamaProvider::PROVIDER_NAME)?,
            NodeLocality::Local,
            true,
        ));
    }
    if let Some(key) = config.providers().openai_api_key()
        && !catalogs.openai.is_empty()
    {
        let adapter: Arc<dyn agentkube_providers::ModelProvider> = Arc::new(OpenAiProvider::new(
            config.providers().openai_base_url(),
            key.expose(),
            catalogs.openai.clone(),
        )?);
        registry.register(adapter, profiles(&catalogs.openai, DataResidency::Remote)?)?;
        providers.push(ProviderDescriptor::new(
            ProviderName::new(OpenAiProvider::PROVIDER_NAME)?,
            NodeLocality::Cloud,
            false,
        ));
    }
    if providers.is_empty() {
        eprintln!(
            "providers: no models registered; tasks will stay queued until a provider is configured"
        );
    }
    Ok((registry, providers))
}

fn profiles(
    catalog: &[(ModelName, ModelCapabilities)],
    residency: DataResidency,
) -> Result<Vec<(ModelName, ModelRoutingProfile)>, Box<dyn Error>> {
    catalog
        .iter()
        .map(|(name, _)| {
            Ok((
                name.clone(),
                ModelRoutingProfile::new(5_000, "30s".parse()?, residency)?,
            ))
        })
        .collect()
}
