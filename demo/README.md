# Demos de AgentKube

## Equipo de agentes con Ollama (inferencia real)

[`ollama-team/`](ollama-team/) contiene cuatro agentes especializados, dos
deployments y tareas listas para ejecutar contra Ollama mediante el operador
durable:

```bash
ollama pull llama3.2:3b
ollama pull qwen2.5-coder:7b
./demo/ollama-team/run.sh
```

Consulta [`ollama-team/README.md`](ollama-team/README.md) para ejecución
manual, configuración y comandos de inspección.

## Tres clusters de API (recorrido del control plane)

Each `agentkube-api` process is a self-contained cluster with isolated
in-memory state. This demo runs three of them on one machine and operates
all three with `akctl --server <url>`:

| Cluster      | URL                   | Story                              |
|--------------|-----------------------|------------------------------------|
| `ollama-local` | http://127.0.0.1:18081 | Local models only (`ollama`)       |
| `cloud`        | http://127.0.0.1:18082 | Hosted providers (`openai`, `anthropic`) |
| `edge`         | http://127.0.0.1:18083 | Tiny on-device footprint           |

> Esta demo multinodo usa `agentkube-api`: almacena y sirve los recursos, pero
> no ejecuta inferencia. Para tareas reales con Ollama usa `ollama-team/`.

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
