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
| `agentkube-tasks` | Task requirements, budgets, retries, results, and lifecycle state | Implemented |
| `agentkube-storage` | Async repository ports, optimistic concurrency, and in-memory storage | Implemented |
| `agentkube-sqlite` | Durable SQLite repositories with WAL and crash-safe boot recovery | Implemented |
| `agentkube-queue` | Priority dispatch, delayed delivery, leases, and recovery | Implemented |
| `agentkube-providers` | Model inference contracts, normalized errors, streaming, and capabilities | Implemented |
| `agentkube-router` | Health, privacy, capability, cost-aware routing, and failover | Implemented |
| `agentkube-workers` | Atomic execution, runtime integration, heartbeats, and settlement | Implemented |
| `agentkube-scheduler` | Constraint filtering, explainable scoring, and deterministic placement | Implemented |
| `agentkube-controllers` | Idempotent deployment and agent-status reconciliation | Implemented |
| `agentkube-api` | Versioned HTTP CRUD, pagination, health checks, and task dispatch | Implemented |
| `agentkube-cli` | `akctl` HTTP client: health, apply, get/describe, delete, scale, tables/JSON/YAML | Implemented |

The complete technical vision is documented in
[docs/architecture.md](docs/architecture.md).
The implementation contract for the CLI module is documented in
[docs/cli-module.md](docs/cli-module.md).

## Development

AgentKube requires Rust 1.88 or newer.

```bash
git clone https://github.com/devdanielvaldez/agentkube.git
cd agentkube

cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
```

Run the local in-memory API server:

```bash
cargo run -p agentkube-api --bin agentkube-api
```

The server listens on `127.0.0.1:8080` by default. Settings can be overridden
through typed `AGENTKUBE_*` environment variables, such as
`AGENTKUBE_CONTROL_PLANE__BIND_ADDRESS`.

The project is organized as a Cargo workspace. Shared functionality lives in
small crates with explicit responsibilities, infrastructure-independent domain
types, documented public APIs, and tests at module and public-contract levels.

End-to-end CLI smoke test against a throwaway server: `./scripts/smoke-cli.sh`.

## Install

Prebuilt `akctl` + `agentkube-api` installers for macOS, Linux, and Windows:

```bash
# Homebrew (macOS/Linux) — fully qualified: bare `agentkube` resolves to an unrelated cask
brew tap devdanielvaldez/agentkube https://github.com/devdanielvaldez/agentkube
brew install --formula devdanielvaldez/agentkube/agentkube
```

```bash
# macOS / Linux (script)
curl -fsSL https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.sh | bash
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.ps1 | iex
```

Full guide (pinned versions, manual downloads, checksums, source build,
uninstall): [docs/install.md](docs/install.md).
`akctl` notifies on stderr when a newer release exists (daily cached check;
`AGENTKUBE_NO_UPDATE_CHECK=1` disables it).

## CLI usage (`akctl`)

```bash
cargo run -p agentkube-api --bin agentkube-api &
cargo run -p agentkube-cli --bin akctl -- status
cargo run -p agentkube-cli --bin akctl -- health
cargo run -p agentkube-cli --bin akctl -- apply -f agents.yaml
cargo run -p agentkube-cli --bin akctl -- get agents
cargo run -p agentkube-cli --bin akctl -- get deployments
cargo run -p agentkube-cli --bin akctl -- get tasks
cargo run -p agentkube-cli --bin akctl -- describe agent backend-agent
cargo run -p agentkube-cli --bin akctl -- delete agent backend-agent
cargo run -p agentkube-cli --bin akctl -- scale deployment workers --replicas 4
```

Useful flags: `--server http://127.0.0.1:8080` (or `AGENTKUBE_SERVER`),
`--timeout 30s`, `-o table|json|yaml`, `--no-color`, `-v`, and
`apply --dry-run=client` for client-side validation without requests.
`describe` defaults to YAML while `get` honors the requested output mode.
See `akctl --help` for the full contract.

## Contributing

AgentKube is being built in public. Architecture discussions, focused issues,
and well-scoped pull requests are welcome. Before proposing a large change,
please review the architecture document and open an issue to discuss its module
boundaries and operational impact.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
