# AgentKube

> The orchestration layer for autonomous AI agents.

AgentKube is an open-source control plane for deploying, scheduling, scaling,
securing, and observing autonomous AI agents across models, runtimes, and
infrastructure.

The project applies proven orchestration principles—declarative resources,
desired-state reconciliation, scheduling, isolation, policy enforcement, and
observability—to AI-native workloads.

> [!IMPORTANT]
> AgentKube is in early development. Its public APIs and resource definitions
> are not stable yet, and it is not ready for production workloads.

## Why AgentKube?

Running one agent is straightforward. Operating hundreds introduces a
different class of problems:

- matching tasks to agents, models, and infrastructure;
- recovering work after worker or provider failures;
- enforcing permissions, privacy requirements, and human approvals;
- controlling token, compute, and financial budgets;
- coordinating shared context, memory, tools, and workspaces;
- tracing decisions and actions across multi-agent workflows;
- routing around provider outages, rate limits, and latency spikes.

AgentKube treats agents as schedulable, stateful, policy-controlled workloads.

```text
Applications
     │
     ▼
AgentKube Control Plane
     │
     ├── Scheduler
     ├── Controllers
     ├── Policies
     ├── Model Router
     └── Task Queue
              │
       ┌──────┼──────┐
       ▼      ▼      ▼
    Workers Workers Workers
       │      │      │
       ▼      ▼      ▼
   OpenAI  Anthropic Local models
```

## Design principles

- **Provider agnostic:** models are selected through adapters instead of being
  embedded into the control plane.
- **Runtime agnostic:** agents may use different SDKs and execution engines.
- **Infrastructure agnostic:** workers may run locally, in containers, on
  Kubernetes, on bare metal, or on GPU nodes.
- **Declarative:** agents and tasks are described as versioned resources.
- **Secure by default:** workloads receive explicit capabilities and bounded
  access.
- **Cost aware:** tokens, model calls, compute, and money are managed resources.
- **Observable:** task execution is traceable across agents, tools, and models.
- **Self-healing:** retries, checkpoints, leases, and reconciliation recover
  workloads when possible.

## Current status

The repository currently contains the foundational Rust workspace:

| Crate | Responsibility | Status |
|---|---|---|
| `agentkube-core` | Typed IDs, resource metadata, names, namespaces, and versions | Implemented |
| `agentkube-protocol` | Declarative documents, API errors, pagination, and watch events | Implemented |
| `agentkube-config` | Typed layered configuration with validation | Implemented |
| `agentkube-agents` | Agent definitions, deployments, policies, and lifecycle state | Implemented |

The next milestone is the task domain, followed by persistence, task queues,
providers, workers, scheduling, reconciliation, API, and CLI.

The complete technical vision is documented in
[docs/architecture.md](docs/architecture.md).

## Development

AgentKube requires Rust 1.85 or newer.

```bash
git clone https://github.com/devdanielvaldez/agentkube.git
cd agentkube

cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
```

The project is organized as a Cargo workspace. Shared functionality lives in
small crates with explicit responsibilities, infrastructure-independent domain
types, documented public APIs, and tests at module and public-contract levels.

## Planned CLI experience

```bash
agentkube init
akctl apply -f agents.yaml
akctl get agents
akctl get tasks
akctl logs backend-agent --follow
akctl scale backend-agent --replicas=10
```

These commands represent the intended interface and are not implemented yet.

## Contributing

AgentKube is being built in public. Architecture discussions, focused issues,
and well-scoped pull requests are welcome. Before proposing a large change,
please review the architecture document and open an issue to discuss its module
boundaries and operational impact.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
