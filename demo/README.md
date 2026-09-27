# Demo: three local clusters

Each `agentkube-api` process is a self-contained cluster with isolated
in-memory state. This demo runs three of them on one machine and operates
all three with `akctl --server <url>`:

| Cluster      | URL                   | Story                              |
|--------------|-----------------------|------------------------------------|
| `ollama-local` | http://127.0.0.1:18081 | Local models only (`ollama`)       |
| `cloud`        | http://127.0.0.1:18082 | Hosted providers (`openai`, `anthropic`) |
| `edge`         | http://127.0.0.1:18083 | Tiny on-device footprint           |

> The provider/model names in the manifests are declarative labels: the demo
> API server stores and serves them, but no real inference runs. Point them at
> your Ollama models (e.g. `llama3.1`, `qwen2.5-coder`) so the specs already
> match your local setup when workers arrive.

## Run it

Requires the binaries (`brew`, `install.sh`, or `cargo build -p agentkube-cli -p agentkube-api`):

```bash
./demo/demo.sh
```

The script starts the three servers, applies each cluster's manifests from
`demo/clusters/<name>/`, shows the inventory of every cluster, scales the
edge deployment, prints one detail view, and stops the servers. Logs land in
`demo/.logs/` (git-ignored; delete freely).

Keep the servers up to explore manually:

```bash
./demo/demo.sh --keep
akctl --server http://127.0.0.1:18081 get tasks -o json
akctl --server http://127.0.0.1:18082 get deployments -o yaml
akctl --server http://127.0.0.1:18083 scale deployment edge-helpers --replicas 3
```

State is in memory: restarting a server resets its cluster. Re-run
`demo.sh` any time for a fresh trio.

## Layout

```text
demo/
├── README.md            # this guide
├── demo.sh              # orchestrator (start, apply, tour, stop)
└── clusters/
    ├── ollama-local/    # agent.yaml, deployment.yaml, task.yaml
    ├── cloud/           # agent.yaml, deployment.yaml, task.yaml
    └── edge/            # agent.yaml, deployment.yaml, task.yaml
```
