#!/usr/bin/env bash
# Builds and runs the development AgentKube binaries against Ollama.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CARGO_BIN="${CARGO_BIN:-cargo}"
TARGET_DIR="${AGENTKUBE_DEMO_TARGET_DIR:-$REPO_DIR/target}"
AKCTL="${AKCTL:-$TARGET_DIR/debug/akctl}"
OPERATOR_BIN="${OPERATOR_BIN:-$TARGET_DIR/debug/agentkube-operator}"
SERVER="${AGENTKUBE_SERVER:-http://127.0.0.1:18080}"
OLLAMA_BASE_URL="${OLLAMA_BASE_URL:-http://127.0.0.1:11434}"
DATA_DIR="${AGENTKUBE_DEMO_DATA_DIR:-$REPO_DIR/demo/.data/ollama-team}"
LOG_DIR="${AGENTKUBE_DEMO_LOG_DIR:-$REPO_DIR/demo/.logs}"
OPERATOR_PID=""

cleanup() {
  if [ -n "$OPERATOR_PID" ]; then
    kill "$OPERATOR_PID" 2>/dev/null || true
    wait "$OPERATOR_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

command -v "$CARGO_BIN" >/dev/null 2>&1 || {
  echo "No se encontró Cargo. Instala Rust con rustup para ejecutar la demo en modo dev." >&2
  exit 1
}
command -v curl >/dev/null 2>&1 || {
  echo "Se requiere curl para comprobar Ollama." >&2
  exit 1
}

if ! curl -fsS "$OLLAMA_BASE_URL/api/tags" >/dev/null; then
  echo "Ollama no responde en $OLLAMA_BASE_URL." >&2
  echo "Ejecuta 'ollama serve' y descarga los modelos indicados en README.md." >&2
  exit 1
fi

echo "Compilando AgentKube en modo dev..."
(
  cd "$REPO_DIR"
  CARGO_TARGET_DIR="$TARGET_DIR" "$CARGO_BIN" build \
    -p agentkube-cli \
    -p agentkube-operator
)

command -v "$AKCTL" >/dev/null 2>&1 || {
  echo "La compilación no produjo el cliente esperado: $AKCTL" >&2
  exit 1
}
command -v "$OPERATOR_BIN" >/dev/null 2>&1 || {
  echo "La compilación no produjo el operador esperado: $OPERATOR_BIN" >&2
  exit 1
}

mkdir -p "$DATA_DIR" "$LOG_DIR"

case "$SERVER" in
  http://127.0.0.1:*) BIND_ADDRESS="127.0.0.1:${SERVER##*:}" ;;
  http://localhost:*) BIND_ADDRESS="127.0.0.1:${SERVER##*:}" ;;
  *)
    echo "run.sh solo puede arrancar el operador en una URL HTTP local: $SERVER" >&2
    exit 2
    ;;
esac

AGENTKUBE_CONTROL_PLANE__BIND_ADDRESS="$BIND_ADDRESS" \
AGENTKUBE_STORAGE__DATA_DIR="$DATA_DIR" \
AGENTKUBE_PROVIDERS__OLLAMA_BASE_URL="$OLLAMA_BASE_URL" \
AGENTKUBE_OPERATOR__DISPATCH_INTERVAL="500ms" \
AGENTKUBE_OPERATOR__RECONCILE_INTERVAL="1s" \
AGENTKUBE_WORKER__CONCURRENCY="2" \
AGENTKUBE_WORKER__LEASE_TIMEOUT="10m" \
"$OPERATOR_BIN" >"$LOG_DIR/ollama-team-operator.log" 2>&1 &
OPERATOR_PID=$!

for _ in $(seq 1 60); do
  if AGENTKUBE_NO_UPDATE_CHECK=1 "$AKCTL" --server "$SERVER" health >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$OPERATOR_PID" 2>/dev/null; then
    echo "El operador terminó durante el arranque. Revisa $LOG_DIR/ollama-team-operator.log" >&2
    exit 1
  fi
  sleep 0.25
done

if ! AGENTKUBE_NO_UPDATE_CHECK=1 "$AKCTL" --server "$SERVER" health >/dev/null 2>&1; then
  echo "El operador no quedó listo. Revisa $LOG_DIR/ollama-team-operator.log" >&2
  exit 1
fi

export AGENTKUBE_NO_UPDATE_CHECK=1

# AgentTask is create-only. Removing the known demo tasks makes reruns safe.
for task in plan-local-api implement-health-handler review-auth-design write-agentkube-quickstart; do
  "$AKCTL" --server "$SERVER" delete task "$task" >/dev/null 2>&1 || true
done

"$AKCTL" --server "$SERVER" apply -f "$SCRIPT_DIR/agents.yaml"
"$AKCTL" --server "$SERVER" apply -f "$SCRIPT_DIR/deployments.yaml"
"$AKCTL" --server "$SERVER" apply -f "$SCRIPT_DIR/tasks.yaml"

echo
echo "Demo activa en $SERVER (PID $OPERATOR_PID)"
echo "Logs: $LOG_DIR/ollama-team-operator.log"
echo
"$AKCTL" --server "$SERVER" status
echo
echo "Consulta el progreso con:"
echo "  $AKCTL --server $SERVER get tasks"
echo "  $AKCTL --server $SERVER logs implement-health-handler"
echo "  $AKCTL --server $SERVER logs -f plan-local-api"
echo "  $SCRIPT_DIR/logs.sh  # espera y muestra la salida de las cuatro tareas"
echo "  tail -f $LOG_DIR/ollama-team-operator.log"
echo
echo "Pulsa Ctrl-C para detener el operador."
wait "$OPERATOR_PID"
