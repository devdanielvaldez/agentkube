# AgentKube

> **The orchestration layer for autonomous AI agents.**

**AgentKube** es una plataforma open source de orquestación para agentes de inteligencia artificial inspirada en los principios de Kubernetes.

El objetivo es permitir a desarrolladores y empresas **desplegar, escalar, distribuir, aislar, monitorear y coordinar agentes de IA** independientemente del modelo, proveedor, runtime o infraestructura donde se ejecuten.

```bash
agentkube init
akctl apply -f agents.yaml
akctl get agents
akctl get tasks
akctl logs backend-agent
akctl scale backend-agent --replicas=10
akctl dashboard
```

---

# 1. Visión

Actualmente es relativamente sencillo crear un agente:

```text
User
  ↓
Planner Agent
  ↓
Coding Agent
  ↓
Testing Agent
  ↓
Reviewer Agent
```

El problema aparece cuando una organización necesita ejecutar decenas, cientos o miles de agentes:

```text
                 ┌─ 20 Coding Agents
                 │
1000 tasks ──────┼─ 10 Research Agents
                 │
                 ├─ 5 Browser Agents
                 │
                 ├─ 5 Testing Agents
                 │
                 └─ 3 Reviewer Agents
```

Surgen preguntas nuevas:

- ¿Quién decide qué agente ejecuta cada tarea?
- ¿Qué ocurre si un agente falla?
- ¿Cómo recuperamos su trabajo?
- ¿Qué ocurre si un proveedor de modelos deja de responder?
- ¿Qué ocurre cuando se alcanza un rate limit?
- ¿Cómo evitamos que un agente consuma demasiado dinero?
- ¿Qué agentes pueden acceder a producción?
- ¿Cómo comparten contexto varios agentes?
- ¿Cómo escalamos automáticamente?
- ¿Cómo distribuimos tareas?
- ¿Cómo ejecutamos modelos locales?
- ¿Cómo auditamos las acciones realizadas?
- ¿Cómo detenemos agentes atrapados en loops?
- ¿Cómo combinamos OpenAI, Anthropic, Gemini, Llama y otros proveedores?

**AgentKube pretende resolver estos problemas.**

---

# 2. Tesis técnica

La idea central del proyecto es:

> **AI agents should be treated as schedulable, stateful, policy-controlled computational workloads.**

Los agentes dejan de ser simplemente funciones dentro de una aplicación y pasan a convertirse en **workloads administrados por una infraestructura especializada**.

AgentKube combina:

```text
Agent orchestration
        +
Model orchestration
        +
Compute orchestration
        +
Context orchestration
        +
Tool orchestration
        +
Policy enforcement
        +
Cost orchestration
```

---

# 3. Relación conceptual con Kubernetes

Kubernetes administra:

```text
Containers
Pods
Nodes
Services
Jobs
Deployments
```

AgentKube administraría:

```text
Agents
AgentPods
AgentNodes
Tasks
Workflows
AgentPools
Tools
Memories
Budgets
Policies
```

Equivalencias conceptuales:

| Kubernetes | AgentKube |
|---|---|
| Container | Agent |
| Pod | AgentPod |
| Deployment | AgentDeployment |
| Job | AgentTask / AgentJob |
| CronJob | AgentCronJob |
| Node | AgentNode |
| Scheduler | AgentScheduler |
| Service | AgentService |
| ConfigMap | AgentConfig |
| Secret | AgentSecret |
| HPA | AgentAutoscaler |
| NetworkPolicy | AgentPolicy |
| Namespace | AgentNamespace |
| CRD | AgentDefinition |

AgentKube **no necesita ser un fork de Kubernetes**.

La primera versión tendría su propio control plane inspirado en sus conceptos.

Posteriormente podría existir un **AgentKube Kubernetes Operator** para desplegar AgentKube sobre clusters Kubernetes tradicionales.

---

# 4. Arquitectura general

```text
                         AGENTKUBE

                     ┌───────────────┐
                     │     CLI       │
                     │    akctl      │
                     └───────┬───────┘
                             │
                             ▼
                  ┌─────────────────────┐
                  │     API SERVER      │
                  └─────────┬───────────┘
                            │
             ┌──────────────┼──────────────┐
             ▼              ▼              ▼
        Scheduler       Controller      Registry
             │              │              │
             └──────────────┼──────────────┘
                            ▼
                    ┌───────────────┐
                    │  Task Queue   │
                    └───────┬───────┘
                            │
          ┌─────────────────┼─────────────────┐
          ▼                 ▼                 ▼
     Agent Node        Agent Node        Agent Node
          │                 │                 │
    ┌─────┴─────┐     ┌─────┴─────┐     ┌─────┴─────┐
    │ AgentPods │     │ AgentPods │     │ AgentPods │
    └─────┬─────┘     └───────────┘     └───────────┘
          │
     ┌────┼──────────────┐
     ▼    ▼              ▼
   OpenAI Claude        Llama
```

---

# 5. Control Plane

El Control Plane constituye el cerebro de AgentKube.

```text
agentkube-control-plane
```

Inicialmente contendría:

```text
API Server
Scheduler
Controller Manager
Task Manager
Model Router
Budget Controller
Policy Engine
Agent Registry
Event Bus
```

El API Server sería el principal punto de entrada.

Ejemplos:

```http
POST /api/v1/agents
POST /api/v1/tasks
POST /api/v1/workflows

GET /api/v1/agents
GET /api/v1/tasks
GET /api/v1/nodes
GET /api/v1/events
```

---

# 6. Agent Definition

Los agentes pueden definirse declarativamente.

```yaml
apiVersion: agentkube.ai/v1
kind: Agent

metadata:
  name: backend-developer

spec:
  role: software-engineer

  model:
    provider: openai
    model: gpt-x

  instructions: |
    You are an expert backend developer.

  tools:
    - github
    - terminal
    - postgres

  resources:
    maxTokens: 100000
    maxCost: 2.00
    timeout: 20m

  permissions:
    network: true
    filesystem: workspace
    production: false

  memory:
    type: persistent

  replicas: 3
```

Aplicación:

```bash
akctl apply -f backend-agent.yaml
```

---

# 7. AgentDeployment

Un `AgentDeployment` mantiene un estado deseado de agentes.

```yaml
apiVersion: agentkube.ai/v1
kind: AgentDeployment

metadata:
  name: coding-agents

spec:
  replicas: 10

  selector:
    role: developer

  template:
    model:
      strategy: auto

    capabilities:
      - coding
      - terminal
      - git

    resources:
      maxCostPerTask: 1.50

    restartPolicy: OnFailure
```

AgentKube intenta mantener:

```text
desired agents = 10
actual agents = 10
```

Si tres workers desaparecen:

```text
desired = 10
actual = 7

Controller
   ↓

create 3
```

Esto implementa el patrón de:

> **Desired State + Reconciliation**

---

# 8. AgentTask

`AgentTask` representa una unidad de trabajo.

```yaml
apiVersion: agentkube.ai/v1
kind: AgentTask

metadata:
  name: fix-login-bug

spec:
  objective: |
    Investigate and fix GitHub issue #381.

  requirements:
    capabilities:
      - coding
      - github
      - testing

  priority: high

  budget:
    maxCost: 3

  timeout: 30m
```

Ejecutar:

```bash
akctl apply -f task.yaml
```

El scheduler seleccionará el agente adecuado.

---

# 9. Agent Scheduler

El Scheduler será una de las partes centrales del sistema.

Recibe:

```text
TASK

"Fix React authentication bug."
```

Analiza:

```text
required capability = coding
language = TypeScript
estimated complexity = medium
context size = 150k
budget = $2
```

Supongamos que existen:

```text
Agent A
OpenAI
coding=0.95
cost=$$$
available=yes

Agent B
Claude
coding=0.97
cost=$$$$
available=yes

Agent C
Llama
coding=0.78
cost=$
available=yes
```

El Scheduler podría calcular:

```text
score =
 quality * 0.35
+ capability * 0.25
+ availability * 0.15
+ latency * 0.10
+ costEfficiency * 0.15
```

Los pesos serían configurables.

---

# 10. Scheduling Constraints

Los workloads pueden declarar restricciones.

```yaml
scheduling:
  require:
    capabilities:
      - coding

  prefer:
    provider:
      - anthropic
      - openai

  avoid:
    provider:
      - local

  maxLatency: 10s
  maxCost: 1.00
```

Esto permite scheduling consciente de:

- capacidad;
- coste;
- modelo;
- proveedor;
- privacidad;
- latencia;
- hardware;
- disponibilidad.

---

# 11. Model Router

Un principio importante será separar:

```text
Agent
```

de:

```text
Model
```

Un agente no debería estar obligatoriamente asociado permanentemente a GPT, Claude o Llama.

Podría definirse:

```yaml
model:
  strategy: auto

  providers:
    - openai
    - anthropic
    - google
    - local

  optimizeFor:
    - quality
    - cost
    - latency
```

Arquitectura:

```text
Backend Agent
      ↓
Model Router
      ↓
Best available model
```

El router podría considerar:

```text
Task complexity
Model capabilities
Historical success
Cost
Latency
Context window
Provider health
Rate limits
Privacy
```

---

# 12. Provider Failover

Ejemplo:

```text
Task
 ↓
OpenAI
 ↓
429 Rate Limit
```

AgentKube podría automáticamente utilizar:

```text
OpenAI
  X

Claude
  ↓

continue task
```

Configuración:

```yaml
model:
  primary:
    provider: openai

  fallback:
    - anthropic
    - google
    - local
```

---

# 13. AgentPods

Un `AgentPod` agrupa agentes relacionados que comparten recursos.

```yaml
kind: AgentPod

metadata:
  name: feature-team

spec:
  agents:
    - name: architect
      role: architect

    - name: developer
      role: developer

    - name: tester
      role: qa

    - name: reviewer
      role: reviewer
```

Conceptualmente:

```text
AgentPod

┌────────────────────────────┐
│                            │
│ Architect                  │
│      ↓                     │
│ Developer                  │
│      ↓                     │
│ Tester                     │
│      ↓                     │
│ Reviewer                   │
│                            │
│ Shared workspace           │
│ Shared memory              │
│ Shared context             │
└────────────────────────────┘
```

---

# 14. Agent Workflows

Los agentes pueden coordinarse mediante workflows.

```yaml
kind: AgentWorkflow

metadata:
  name: build-feature

spec:
  steps:
    - name: plan
      agent: architect

    - name: implement
      agent: developer
      dependsOn:
        - plan

    - name: test
      agent: tester
      dependsOn:
        - implement

    - name: review
      agent: reviewer
      dependsOn:
        - test
```

Resultado:

```text
Architect
   ↓
Developer
   ↓
Tester
   ↓
Reviewer
```

También pueden existir DAGs:

```text
                    Architect
                       │
              ┌────────┴────────┐
              ▼                 ▼
          Frontend           Backend
              │                 │
              └────────┬────────┘
                       ▼
                    Tester
                       │
                       ▼
                   Reviewer
```

---

# 15. Agent-to-Agent Communication

AgentKube debe proporcionar comunicación independiente del proveedor.

Ejemplo:

```typescript
await context.send({
  to: "tester",
  type: "TASK",
  payload: {
    branch: "feature/login"
  }
});
```

Internamente:

```text
Agent A
 ↓
Event Bus
 ↓
Agent B
```

El objetivo es permitir que agentes construidos utilizando runtimes diferentes puedan colaborar.

---

# 16. AgentService

Un `AgentService` proporciona descubrimiento y balanceo.

En lugar de conocer:

```text
developer-agent-183
developer-agent-281
developer-agent-929
```

una aplicación utiliza:

```text
agent://backend-developer
```

Configuración:

```yaml
kind: AgentService

metadata:
  name: backend-developer

spec:
  selector:
    role: backend

  loadBalancing:
    strategy: least-busy
```

Arquitectura:

```text
agent://backend-developer

          ↓

      Load Balancer

     /      |      \
Agent 1   Agent 2   Agent 3
```

---

# 17. AgentNamespaces

Las organizaciones pueden separar workloads:

```text
company/

├── production
├── development
├── research
└── support
```

CLI:

```bash
akctl namespace use production
```

Cada namespace podría aislar:

```text
Agents
Secrets
Memory
Tools
Budgets
Policies
Workflows
```

---

# 18. Tool Registry

AgentKube incluiría un registro de herramientas.

```bash
akctl tool install github
akctl tool install postgres
akctl tool install browser
```

Con soporte futuro para MCP.

Ejemplo:

```yaml
tools:
  - name: github

    permissions:
      repositories:
        - company/backend

      actions:
        - read
        - branch
        - pull-request
```

---

# 19. Agent Security Context

Inspirado en `SecurityContext`.

```yaml
securityContext:
  filesystem:
    readOnly: false

  network:
    outbound: restricted

  shell:
    enabled: true

  privileged: false

  production:
    access: false

  approvalRequired:
    - database.write
    - deployment.production
```

---

# 20. AgentPolicy

Las políticas controlan lo que un agente puede realizar.

```yaml
kind: AgentPolicy

metadata:
  name: developer-policy

spec:
  allow:
    - github.read
    - github.branch
    - github.pull-request

  requireApproval:
    - github.merge
    - deployment.production

  deny:
    - database.drop
    - secrets.read
```

---

# 21. Secret Management

Nunca deberían introducirse directamente en prompts:

```text
OPENAI_API_KEY
GITHUB_TOKEN
DATABASE_PASSWORD
```

Crear secreto:

```bash
akctl secret create openai \
  --from-env OPENAI_API_KEY
```

Uso:

```yaml
secrets:
  - openai
  - github
```

Idealmente el agente obtiene acceso a una **capability**, no al valor del secreto.

---

# 22. Budget Controller

AgentKube debe tratar el dinero como un recurso computacional.

```yaml
budget:
  perTask: 2
  daily: 100
  monthly: 2000
```

El controller registra:

```text
Tokens
API calls
Tool calls
Compute
Storage
```

Cuando:

```text
cost >= budget
```

el workload puede pasar a:

```text
AgentTask → Suspended
```

---

# 23. Resource Requests

Los agentes pueden solicitar recursos AI-native.

```yaml
resources:
  requests:
    context: 50000
    reasoning: medium

  limits:
    tokens: 200000
    cost: 3
    runtime: 30m
```

Equivalente conceptual a:

```text
CPU
RAM
```

pero adaptado a workloads de IA.

---

# 24. Agent Autoscaler

Ejemplo:

```yaml
kind: AgentAutoscaler

metadata:
  name: coding-autoscaler

spec:
  target:
    deployment: coding-agents

  minReplicas: 2
  maxReplicas: 50

  metrics:
    pendingTasks:
      target: 5
```

Si existen:

```text
2 agents
200 pending tasks
```

AgentKube escala:

```text
2 → 10 → 20
```

Cuando baja la demanda:

```text
20 → 5 → 2
```

---

# 25. Autoscaling Inteligente

Las decisiones pueden utilizar:

```text
Queue depth
Token consumption
Task complexity
Latency
Cost
Provider rate limits
GPU availability
Model availability
```

Esto permitiría crear un autoscaler específicamente diseñado para IA.

---

# 26. AgentNode

Un Node representa infraestructura capaz de ejecutar agentes.

Ejemplos:

```text
Mac Studio
RTX 5090 server
AWS
DigitalOcean
Vercel
Local PC
```

Registrar:

```bash
agentkube node join \
  --server https://control.agentkube.ai
```

Consultar:

```bash
akctl get nodes
```

Resultado:

```text
NAME          GPU       RAM     STATUS

mac-studio    M4 Max    64GB    Ready
gpu-01        RTX5090   64GB    Ready
cloud-01      none      16GB    Ready
```

---

# 27. Local Models

Los workloads pueden solicitar modelos locales.

```yaml
model:
  provider: local

  requirements:
    vram: 16GB

    capabilities:
      - coding
```

Scheduler:

```text
Agent requires GPU
        ↓
find nodes
        ↓

node-01 ❌ no GPU
node-02 ✅ RTX 5090
node-03 ✅ A100

        ↓

schedule node-03
```

---

# 28. Sandboxing

Cada `AgentTask` debería ejecutarse dentro de un entorno aislado.

Opciones:

```text
Docker
Firecracker
gVisor
WASM
```

Para el MVP:

```text
Docker
```

Posteriormente:

```text
Firecracker microVM
```

Arquitectura:

```text
Agent
  ↓
Sandbox

┌─────────────────┐
│ /workspace      │
│ Node            │
│ Python          │
│ Git             │
│ Tools           │
└─────────────────┘
```

---

# 29. Persistent Workspace

Cada tarea puede poseer:

```text
/workspace
```

persistente.

Ejemplo:

```text
Developer modifies code

↓ workspace snapshot

Tester mounts snapshot

↓ tests

Reviewer mounts snapshot
```

Esto permite pasar trabajo entre agentes sin depender únicamente del contexto textual.

---

# 30. Memory

La memoria debe separarse del contexto inmediato del modelo.

```text
Agent Memory

├── Working
├── Episodic
├── Semantic
└── Shared
```

Configuración:

```yaml
memory:
  working:
    ttl: 1h

  persistent:
    provider: postgres

  semantic:
    provider: pgvector

  shared:
    namespace: engineering
```

---

# 31. Context Store

Un servicio especializado podría administrar contexto.

```text
Context Service
```

El sistema puede combinar:

```text
Task
Repository
Previous results
Relevant memories
Dependencies
Documentation
Logs
```

sin enviar necesariamente toda la información al modelo.

---

# 32. Context Compiler

Una de las funcionalidades diferenciadoras.

```text
Task

"Fix login bug."

        ↓

Context Compiler

        ↓

Git repository
Git history
Issue
Logs
Architecture
Dependencies
Previous agent memory

        ↓

Optimized Context

        ↓

Agent
```

Objetivo:

> Proporcionar al agente el mínimo contexto necesario para resolver correctamente una tarea.

Esto puede reducir:

```text
Tokens
Cost
Latency
Noise
```

---

# 33. Observabilidad

Comando:

```bash
akctl top agents
```

Ejemplo:

```text
AGENT       TASKS   TOKENS    COST     LATENCY

backend     281     8.2M      $31.20   4.1s
frontend    192     4.7M      $18.91   3.8s
tester      388     2.1M      $9.20    2.2s
```

---

# 34. Distributed Tracing

Cada tarea tendría:

```text
traceId
```

Ejemplo:

```text
Task #8392

Planner
│
├── Model request
│
├── Github tool
│
└── handoff
     │
     ▼
Developer
│
├── Model request
├── terminal
├── git
└── handoff
     │
     ▼
Tester
```

AgentKube podría utilizar **OpenTelemetry** como formato interno.

---

# 35. Dashboard

El dashboard podría desarrollarse utilizando Next.js.

Ejemplo:

```text
┌───────────────────────────────────────────────┐
│ AgentKube                                     │
├───────────────────────────────────────────────┤
│                                               │
│ Agents        42                              │
│ Running       31                              │
│ Tasks         1,281                           │
│ Failed        8                               │
│                                               │
│ Cost Today              $82.19                │
│ Tokens                  21.8M                 │
│                                               │
├───────────────────────────────────────────────┤
│ Provider status                               │
│                                               │
│ OpenAI       ● Healthy                        │
│ Anthropic    ● Healthy                        │
│ Gemini       ● Healthy                        │
│ Local        ● 3 nodes                        │
└───────────────────────────────────────────────┘
```

---

# 36. Visual Workflow

El dashboard puede representar gráficamente workflows.

```text
             ┌─────────────┐
             │   Planner   │
             └──────┬──────┘
                    │
           ┌────────┴────────┐
           ▼                 ▼
     ┌──────────┐      ┌──────────┐
     │ Frontend │      │ Backend  │
     └────┬─────┘      └────┬─────┘
          │                 │
          └────────┬────────┘
                   ▼
              ┌────────┐
              │ Tester │
              └───┬────┘
                  ▼
              ┌────────┐
              │Reviewer│
              └────────┘
```

Al seleccionar un nodo:

```text
Agent: backend-83

Model:
Claude

Runtime:
12m 31s

Tokens:
82,192

Cost:
$0.81

Tools:
Github
Terminal
Postgres

Status:
Running
```

---

# 37. Logs

Experiencia similar a Kubernetes:

```bash
akctl logs backend-agent-82
```

Resultado:

```text
10:42:01 task received
10:42:02 loading context
10:42:04 model request
10:42:09 tool: github.read
10:42:12 tool: terminal.execute
10:42:22 model request
10:42:27 task completed
```

Streaming:

```bash
akctl logs backend-agent-82 --follow
```

---

# 38. Describe

```bash
akctl describe agent backend-agent-82
```

Resultado:

```text
Name:
backend-agent-82

Status:
Running

Model:
anthropic/...

Node:
gpu-node-2

Tasks:
18

Tokens:
892,192

Cost:
$8.29

Memory:
persistent

Tools:
github
terminal

Created:
2h ago
```

---

# 39. CLI

La CLI principal sería:

```text
akctl
```

Ejemplos:

```bash
akctl get agents

akctl get tasks

akctl get nodes

akctl get workflows

akctl describe agent coder-28

akctl logs coder-28

akctl delete agent coder-28

akctl scale deployment coder --replicas 10

akctl apply -f agents.yaml
```

---

# 40. SDK

Paquete:

```bash
npm install @agentkube/sdk
```

Ejemplo:

```typescript
import { AgentKube } from "@agentkube/sdk";

const client = new AgentKube();

const task = await client.tasks.create({
  objective: "Fix Github issue #382",

  capabilities: [
    "coding",
    "github"
  ],

  budget: {
    maxCost: 2
  }
});
```

---

# 41. Model Providers

La arquitectura debe utilizar adapters.

```text
packages/providers/

├── openai
├── anthropic
├── google
├── ollama
├── openrouter
├── vercel-ai
└── custom
```

Interface:

```typescript
interface ModelProvider {
  generate(request): Promise<Response>;

  stream(request): AsyncIterable<Event>;

  health(): Promise<Health>;

  estimateCost(request): number;

  capabilities(): Capability[];
}
```

Esto evita acoplar el core a un proveedor determinado.

---

# 42. Agent Runtimes

Debe existir una separación entre:

```text
Model Provider
```

y:

```text
Agent Runtime
```

AgentKube podría ejecutar agentes creados utilizando:

```text
OpenAI Agents SDK
Vercel AI SDK
Claude Agent SDK
LangGraph
Custom Runtime
```

AgentKube los orquesta sin importar su implementación interna.

---

# 43. Agent Runtime Interface

Ejemplo:

```typescript
interface AgentRuntime {
  start(config): Promise<AgentHandle>;

  execute(task): Promise<TaskResult>;

  pause(): Promise<void>;

  resume(): Promise<void>;

  terminate(): Promise<void>;

  health(): Promise<Health>;
}
```

---

# 44. Self-Healing

Ejemplo:

```text
AgentTask

Running
 ↓
Agent crashes
```

Controller:

```text
heartbeat missing

↓

mark unhealthy

↓

snapshot state

↓

schedule replacement

↓

restore state

↓

continue
```

---

# 45. Checkpoints

Durante una tarea:

```text
Step 1
↓
checkpoint

Step 2
↓
checkpoint

Step 3
↓
CRASH
```

Nuevo agente:

```text
load checkpoint #2

↓

continue Step 3
```

Esto permite recuperación sin comenzar completamente desde cero.

---

# 46. Dead Agent Detection

Cada worker envía heartbeats.

```text
worker → control plane

agent-281 alive
agent-282 alive
agent-283 alive
```

Si desaparece:

```text
agent-282

last heartbeat:
45 seconds ago

↓

UNHEALTHY
```

---

# 47. Retry Policies

Configuración:

```yaml
retryPolicy:
  maxAttempts: 3

  backoff: exponential

  retryOn:
    - rate_limit
    - timeout
    - provider_error
```

Errores como:

```text
permission_denied
```

no deberían provocar retries automáticos.

---

# 48. Human Approval

Operaciones sensibles pueden requerir intervención humana.

```yaml
approval:
  requiredFor:
    - deploy.production
    - database.write
    - payment.execute
```

Flujo:

```text
Agent
 ↓
deploy production
 ↓
PAUSED

Waiting for approval
```

Dashboard:

```text
Agent wants to:

Deploy commit 82ac19 to production.

[Approve]

[Reject]
```

---

# 49. Priorities

Los tasks pueden poseer prioridades.

```yaml
priority: critical
```

Niveles:

```text
Critical
High
Normal
Low
Background
```

Eventualmente podría soportarse:

```text
Preemption
```

Un task crítico puede recibir recursos ocupados por uno de baja prioridad.

---

# 50. AgentJob

Para tareas puntuales:

```yaml
kind: AgentJob

spec:
  agent: security-agent

  task: |
    Review repository for vulnerabilities.

  schedule:
    once: true
```

---

# 51. AgentCronJob

Para tareas periódicas:

```yaml
kind: AgentCronJob

metadata:
  name: dependency-review

spec:
  schedule: "0 8 * * *"

  agent:
    role: security

  task: |
    Review dependencies for vulnerabilities
    and open Github issues when necessary.
```

Flujo:

```text
08:00

Security Agent
    ↓
Repository
    ↓
Dependencies
    ↓
Analysis
    ↓
Report / Issue
```

---

# 52. Event-Driven Agents

Los agentes también pueden reaccionar a eventos.

```yaml
trigger:
  github:
    event: pull_request
```

Flujo:

```text
Github PR
   ↓
AgentKube
   ↓
Review Agent
   ↓
PR Review
```

---

# 53. Agent CRDs

El ecosistema podría permitir tipos personalizados.

Ejemplos:

```yaml
kind: SecurityReviewAgent
```

```yaml
kind: CustomerSupportAgent
```

Los plugins podrían registrar nuevos recursos.

---

# 54. AgentKube Hub

Eventualmente podría existir:

```text
AgentKube Hub
```

Conteniendo:

```text
Agents
Tools
Policies
Workflows
Providers
Runtime Adapters
Templates
```

Ejemplo:

```bash
akctl install agent github-reviewer
```

---

# 55. Stack Tecnológico

Una implementación inicial puede utilizar principalmente TypeScript.

## Monorepo

```text
TypeScript
Node.js
pnpm
```

## Backend

```text
NestJS
```

## APIs

```text
REST
WebSocket
gRPC
```

## Database

```text
PostgreSQL
```

## Cache / State

```text
Redis
```

## Messaging

```text
NATS
```

## Observability

```text
OpenTelemetry
Prometheus
Grafana
```

## Containers

```text
Docker
```

## Dashboard

```text
Next.js
React
Tailwind CSS
```

## CLI

```text
Node.js
Commander
```

---

# 56. Monorepo Structure

```text
agentkube/

├── apps/
│   ├── api/
│   ├── dashboard/
│   ├── controller/
│   ├── scheduler/
│   └── worker/
│
├── packages/
│   ├── sdk/
│   ├── cli/
│   ├── core/
│   ├── protocol/
│   ├── runtime/
│   ├── scheduler/
│   ├── security/
│   ├── telemetry/
│   ├── memory/
│   ├── tools/
│   │
│   └── providers/
│       ├── openai/
│       ├── anthropic/
│       ├── google/
│       └── ollama/
│
├── deployments/
│   ├── docker/
│   └── kubernetes/
│
├── examples/
│   ├── coding-team/
│   ├── research-team/
│   └── customer-support/
│
├── docs/
│
└── package.json
```

---

# 57. Entidades principales

Base de datos inicial:

```text
Cluster

Node

AgentDefinition

AgentInstance

AgentDeployment

Task

Workflow

WorkflowRun

Tool

Memory

Checkpoint

Policy

SecretReference

Provider

Model

Trace

Event

Usage

Budget
```

---

# 58. Agent States

```text
Pending
   ↓
Scheduling
   ↓
Starting
   ↓
Ready
   ↓
Running
   ↓
Paused
   ↓
Completed
```

En caso de error:

```text
Running
   ↓
Failed
   ↓
Retrying
   ↓
Running
```

o:

```text
Failed
 ↓
Terminated
```

---

# 59. Task States

```text
PENDING
QUEUED
SCHEDULED
RUNNING
WAITING_TOOL
WAITING_AGENT
WAITING_APPROVAL
COMPLETED
FAILED
CANCELLED
```

---

# 60. REST API

Primera versión:

```text
POST   /v1/agents
GET    /v1/agents
GET    /v1/agents/:id
DELETE /v1/agents/:id

POST   /v1/tasks
GET    /v1/tasks
GET    /v1/tasks/:id
DELETE /v1/tasks/:id

POST   /v1/workflows
GET    /v1/workflows

GET    /v1/nodes

GET    /v1/providers

GET    /v1/models

GET    /v1/events

GET    /v1/metrics
```

---

# 61. MVP

No debe intentarse implementar todo inicialmente.

La primera versión:

```text
Control Plane
      │
      ├── API
      ├── Scheduler
      ├── Controller
      └── Task Queue
              │
              ▼
           Workers
              │
        ┌─────┴──────┐
        ▼            ▼
     OpenAI       Anthropic
```

Debe soportar:

```text
Agent definitions
Tasks
Scheduling
Multi-provider
Workers
Retries
Budgets
Basic policies
Logs
Tracing
Dashboard
CLI
```

---

# 62. Primera Demo Pública

Una demo ideal sería desarrollo autónomo de software.

Usuario:

```text
Build a URL shortener with authentication.
```

AgentKube crea:

```text
Task #882
```

Scheduler:

```text
Planner → OpenAI

Frontend → Claude

Backend → OpenAI

Tests → Claude

Reviewer → OpenAI
```

Dashboard:

```text
                         BUILD #882

                         Planner
                            ✓
                            │
                  ┌─────────┴─────────┐
                  ▼                   ▼

              Frontend             Backend
               Claude              OpenAI
                 ✓                   ●
                  \                 /
                   \               /
                    ▼             ▼
                       Tester
                         ○

                      Reviewer
                         ○
```

Resultado:

```text
✓ Planning
✓ Frontend
✓ Backend
✓ Tests
✓ Review

Duration: 8m 42s

Models used: 3

Agents: 5

Tokens: 182,291

Total cost: $1.82

Pull Request:
#382
```

---

# 63. Demo de Failover

Desactivar deliberadamente un proveedor.

```text
OpenAI

UNAVAILABLE
```

Mientras existen 20 tareas.

AgentKube:

```text
Provider failure detected

↓

Tasks checkpointed

↓

Rescheduling

↓

Anthropic

↓

20/20 running
```

Mensaje de la demo:

> **Kubernetes-style resilience for AI agents.**

---

# 64. Demo de Cluster Híbrido

Cluster:

```text
AgentKube Cluster

Nodes

Daniel-Mac
M4
Ready

Homelab-01
RTX 5090
Ready

Cloud-01
DigitalOcean
Ready
```

Task:

```yaml
privacy: local-only
```

Scheduler:

```text
Cloud providers ❌

Mac ❌ insufficient resources

RTX node ✓

↓

Local Llama
```

Esto demostraría scheduling consciente de:

```text
Hardware
Privacy
Model
Resources
Cost
```

---

# 65. Qué NO desarrollar inicialmente

Evitar comenzar con:

```text
Visual workflow builder
Marketplace
50 providers
Complex vector memory
Custom model inference engine
Billing SaaS
Mobile app
100 integrations
```

Primero construir:

```text
Agent YAML
      ↓
API
      ↓
Scheduler
      ↓
Worker
      ↓
Model
      ↓
Task
      ↓
Result
```

Después:

```text
Retry
Failover
Budget
Scaling
Observability
Security
```

---

# 66. Roadmap

## Phase 1 — Core

```text
Agent specification
Task specification
Control Plane
Scheduler
Worker
OpenAI adapter
Anthropic adapter
PostgreSQL
Redis/NATS
```

## Phase 2 — Reliability

```text
Heartbeats
Retry
Failover
Checkpoint
Reconciliation
Worker recovery
```

## Phase 3 — Security

```text
Permissions
Secrets
Policies
Sandbox
Human approval
```

## Phase 4 — Intelligence

```text
Model routing
Cost optimization
Context compiler
Dynamic scheduling
Historical performance
```

## Phase 5 — Scale

```text
Nodes
Distributed workers
Autoscaling
Local models
GPU scheduling
Hybrid clusters
```

## Phase 6 — Developer Experience

```text
akctl
Dashboard
SDK
Workflow visualization
AgentKube Hub
Templates
Plugins
```

---

# 67. Diferenciación

AgentKube no debería venderse únicamente como:

> Kubernetes for AI Agents.

La innovación debe estar en convertir diferentes recursos de IA en primitivas administrables.

Por ejemplo:

```text
CPU              → Tokens
RAM              → Context
Container         → Agent
Node              → AgentNode
Job               → AgentTask
HPA               → AgentAutoscaler
NetworkPolicy     → AgentPolicy
Scheduler         → AI-aware Scheduler
```

Pero también aparecen recursos que Kubernetes tradicionalmente no necesita entender:

```text
Model quality
Reasoning level
Context window
Token budget
Financial budget
Model availability
Provider rate limits
Tool permissions
Agent memory
Agent reputation
Historical success rate
Privacy requirements
```

Estos serían recursos de primera clase dentro de AgentKube.

---

# 68. Scheduler Inteligente

El objetivo a largo plazo sería que el usuario no necesite seleccionar el modelo.

En lugar de:

```yaml
model:
  provider: openai
  model: some-model
```

podría escribir:

```yaml
model:
  strategy: auto

  requirements:
    quality: high
    maxCost: 1.00
    maxLatency: 10s
```

AgentKube decide automáticamente:

```text
Task
 ↓
Analyze complexity
 ↓
Analyze available models
 ↓
Analyze historical performance
 ↓
Analyze cost
 ↓
Analyze latency
 ↓
Analyze provider health
 ↓
Select model
```

Después de miles de ejecuciones:

```text
Task Type
     ↓
Historical performance
     ↓
Best Model
```

AgentKube podría aprender qué modelos funcionan mejor para cada workload.

---

# 69. Cost-Aware Scheduling

AgentKube debería poder optimizar por presupuesto.

Ejemplo:

```yaml
optimization:
  objective: minimize-cost

  constraints:
    minimumQuality: 0.90
```

El Scheduler buscaría:

> El modelo más barato que históricamente consiga una calidad superior al requisito.

Otro workload:

```yaml
optimization:
  objective: maximum-quality

  budget:
    maxCost: 10
```

Otro:

```yaml
optimization:
  objective: minimum-latency
```

Esto convierte el scheduling en un problema multidimensional.

---

# 70. Privacy-Aware Scheduling

Ejemplo:

```yaml
privacy:
  dataClassification: confidential

  cloudAllowed: false
```

Scheduler:

```text
OpenAI ❌

Anthropic ❌

Gemini ❌

Local Llama ✓
```

Entonces busca un `AgentNode` local compatible.

---

# 71. Capability-Based Scheduling

Los modelos pueden anunciar capacidades.

```yaml
capabilities:
  coding: 0.95
  reasoning: 0.90
  vision: 0.70
  toolCalling: 0.98
```

Los agentes también:

```yaml
requirements:
  coding: high
  toolCalling: required
  vision: false
```

El Scheduler realiza matching.

---

# 72. Cluster Final

El objetivo final sería ejecutar:

```bash
akctl get cluster
```

Resultado:

```text
AGENTKUBE CLUSTER

Nodes             18
Agents            382
Running Tasks     1,281
Queued Tasks      92

Providers

OpenAI            Healthy
Anthropic         Healthy
Gemini            Healthy
Local Llama       8 Nodes

Usage

Tokens today      892M
Cost today        $1,821
Average task      $0.38

Health            99.98%
```

Consultar agentes:

```bash
akctl get agents
```

Resultado:

```text
NAME             REPLICAS   MODEL      STATUS

coder            80         auto       Ready
researcher       40         auto       Ready
tester            50         auto       Ready
reviewer          20         auto       Ready
support          100        auto       Ready
security          10         auto       Ready
```

---

# 73. AgentKube como capa universal

La arquitectura completa podría verse así:

```text
                         APPLICATIONS
                              │
                              ▼
                    ┌───────────────────┐
                    │     AgentKube     │
                    │                   │
                    │   Control Plane   │
                    └─────────┬─────────┘
                              │
       ┌──────────────────────┼──────────────────────┐
       │                      │                      │
       ▼                      ▼                      ▼

   Scheduler              Controller            Policies
       │                      │                      │
       └──────────────────────┼──────────────────────┘
                              │
                              ▼
                         Task Queue
                              │
           ┌──────────────────┼──────────────────┐
           ▼                  ▼                  ▼

       AgentNode          AgentNode          AgentNode
           │                  │                  │
           ▼                  ▼                  ▼

        Agents              Agents              Agents
           │                  │                  │
     ┌─────┼─────┐      ┌─────┼─────┐      ┌─────┼─────┐
     ▼     ▼     ▼      ▼     ▼     ▼      ▼     ▼     ▼

  OpenAI Claude Gemini  Llama Ollama ...   Custom Models
```

---

# 74. Posicionamiento

Tagline:

> **The orchestration layer for autonomous AI agents.**

Alternativa:

> **Deploy, scale and orchestrate AI agents anywhere.**

README inicial:

```text
AgentKube
=========

Deploy, scale and orchestrate AI agents anywhere.

✓ Multi-model
✓ Multi-provider
✓ Distributed
✓ Self-healing
✓ Policy controlled
✓ Cost aware
✓ Observable
✓ Open source

OpenAI • Anthropic • Gemini • Llama • Local
```

---

# 75. Principios del Proyecto

## Provider Agnostic

AgentKube nunca debe depender de un único proveedor.

## Runtime Agnostic

Los agentes pueden utilizar diferentes SDKs y frameworks.

## Infrastructure Agnostic

Los workers pueden ejecutarse:

```text
Local
Cloud
Kubernetes
Docker
Bare Metal
GPU Servers
Edge
```

## Declarative

La infraestructura debe poder definirse mediante YAML.

## Observable

Toda acción debe poder rastrearse.

## Secure by Default

Los agentes reciben únicamente los permisos necesarios.

## Cost Aware

Tokens y dinero son recursos administrados por el sistema.

## Self-Healing

Los workloads deben recuperarse automáticamente cuando sea posible.

## Open Source First

El core debe poder ejecutarse completamente self-hosted.

---

# 76. Objetivo a Largo Plazo

AgentKube debería convertirse en una infraestructura donde una empresa pueda tener:

```text
Thousands of agents

Hundreds of workflows

Multiple model providers

Local GPU clusters

Cloud infrastructure

Shared memory

Tool ecosystems

Security policies

Cost policies
```

administrados desde una única plataforma.

En lugar de que cada aplicación implemente individualmente:

```text
Retries
Routing
Failover
Budgets
Permissions
Scheduling
Scaling
Tracing
Memory
```

AgentKube proporciona estas capacidades como infraestructura.

---

# 77. Visión Final

Hoy una empresa puede tener:

```text
Kubernetes Cluster

1000 Containers
```

En el futuro podría tener:

```text
AgentKube Cluster

1000 Autonomous Agents
```

trabajando simultáneamente en:

```text
Software Development
Customer Support
Research
Security
Data Analysis
DevOps
Infrastructure
Sales
Operations
Monitoring
```

AgentKube sería la capa responsable de decidir:

```text
WHAT agent runs

WHERE it runs

WHEN it runs

WHICH model it uses

WHICH tools it can access

HOW MUCH it can spend

WHAT context it receives

WHAT resources it consumes

WHAT happens if it fails
```

La propuesta fundamental es convertir los agentes autónomos en **recursos computacionales administrables, distribuibles, observables y escalables**.

---

# AgentKube

> **The orchestration layer for autonomous AI agents.**

```text
Agents are workloads.

Models are compute.

Tokens are resources.

Context is memory.

Tools are capabilities.

AgentKube orchestrates everything.
```
