use agentkube_agents::{AgentDefinition, AgentDeployment};
use agentkube_api::{ApiState, router, serve};
use agentkube_config::{ConfigLoader, EnvironmentSource};
use agentkube_queue::InMemoryTaskQueue;
use agentkube_storage::InMemoryResourceRepository;
use agentkube_tasks::AgentTask;
use std::{error::Error, io, sync::Arc};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = ConfigLoader::new()
        .with_source(EnvironmentSource::from_current_process("AGENTKUBE"))
        .load()?;
    let maximum_body_size = usize::try_from(config.control_plane().max_request_body_bytes())
        .map_err(|_| io::Error::other("request body limit exceeds platform usize"))?;
    let state = ApiState::new(
        Arc::new(InMemoryResourceRepository::<AgentDefinition>::new()),
        Arc::new(InMemoryResourceRepository::<AgentDeployment>::new()),
        Arc::new(InMemoryResourceRepository::<AgentTask>::new()),
        Arc::new(InMemoryTaskQueue::new()),
    );
    let listener = TcpListener::bind(config.control_plane().bind_address()).await?;
    println!(
        "AgentKube API {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr()?
    );
    serve(listener, router(state, maximum_body_size)).await?;
    Ok(())
}
