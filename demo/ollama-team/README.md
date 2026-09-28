# Equipo local de agentes con Ollama

Esta demo ejecuta inferencia real mediante `agentkube-operator` y Ollama. Crea
cuatro agentes especializados, dos pools escalables y cuatro tareas locales.

## Requisitos

- Ollama activo en `http://127.0.0.1:11434`.
- Rust y Cargo instalados. No se requiere instalar AgentKube con Homebrew.
- Los modelos usados por los manifiestos:

```bash
ollama pull llama3.2:3b
ollama pull qwen2.5-coder:7b
```

Si ya tienes otros modelos, cambia `spec.model.model` en `agents.yaml` y
`deployments.yaml`. El nombre debe coincidir exactamente con `ollama list`.

## Ejecutar todo

Desde la raíz del repositorio:

```bash
./demo/ollama-team/run.sh
```

El script compila incrementalmente `agentkube-cli` y `agentkube-operator` en
modo debug, comprueba Ollama, inicia el operador desde `target/debug/` en
`127.0.0.1:18080`, aplica los tres manifiestos y muestra el estado. No utiliza
ningún binario instalado con Homebrew. Déjalo ejecutándose y usa otra terminal
para inspeccionar resultados:

La demo limita la concurrencia a dos tareas para no saturar la memoria de una
máquina de desarrollo. Puedes ajustar `AGENTKUBE_WORKER__CONCURRENCY` en
`run.sh` si tu equipo admite más carga.

```bash
./target/debug/akctl --server http://127.0.0.1:18080 get agents
./target/debug/akctl --server http://127.0.0.1:18080 get deployments
./target/debug/akctl --server http://127.0.0.1:18080 get tasks
./target/debug/akctl --server http://127.0.0.1:18080 describe task implement-health-handler
./target/debug/akctl --server http://127.0.0.1:18080 logs implement-health-handler
./target/debug/akctl --server http://127.0.0.1:18080 logs -f plan-local-api
```

`logs` imprime la respuesta final o la causa exacta del fallo. Con `-f` sigue
los cambios de estado hasta que la tarea termina. Los eventos de todas las
ejecuciones también quedan en `demo/.logs/ollama-team-operator.log`:

```bash
tail -f demo/.logs/ollama-team-operator.log
```

Para esperar y mostrar la salida final de las cuatro tareas, consultar un
snapshot sin esperar, o seguir solo una:

```bash
./demo/ollama-team/logs.sh
./demo/ollama-team/logs.sh --snapshot
./demo/ollama-team/logs.sh review-auth-design
./demo/ollama-team/logs.sh --follow plan-local-api
```

No uses `sudo`: el script solo consulta la API local y debe ejecutarse con el
mismo usuario que inició la demo.

El estado SQLite queda en `demo/.data/ollama-team/` y los logs en
`demo/.logs/ollama-team-operator.log`; ambas rutas están ignoradas por Git.
Las cuatro tareas conocidas se recrean al volver a ejecutar el script.

## Ejecutar manualmente

Terminal 1:

```bash
export AGENTKUBE_CONTROL_PLANE__BIND_ADDRESS=127.0.0.1:18080
export AGENTKUBE_STORAGE__DATA_DIR="$PWD/demo/.data/ollama-team"
export AGENTKUBE_PROVIDERS__OLLAMA_BASE_URL=http://127.0.0.1:11434
cargo run -p agentkube-operator --bin agentkube-operator
```

Terminal 2:

```bash
cargo run -p agentkube-cli --bin akctl -- --server http://127.0.0.1:18080 apply -f demo/ollama-team/agents.yaml
cargo run -p agentkube-cli --bin akctl -- --server http://127.0.0.1:18080 apply -f demo/ollama-team/deployments.yaml
cargo run -p agentkube-cli --bin akctl -- --server http://127.0.0.1:18080 apply -f demo/ollama-team/tasks.yaml
cargo run -p agentkube-cli --bin akctl -- --server http://127.0.0.1:18080 get tasks
```

Los requisitos `allowedProviders: [ollama]` y `privacy: localOnly` impiden que
estas tareas se enruten accidentalmente hacia un proveedor remoto.
