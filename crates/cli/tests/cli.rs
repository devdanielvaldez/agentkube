//! Binary-level behaviour for `akctl`: help, version, exit codes, parsing,
//! apply flows, output modes, and error mapping.

use agentkube_agents::{AgentDefinition, AgentDeployment};
use agentkube_api::{ApiState, router, serve};
use agentkube_queue::InMemoryTaskQueue;
use agentkube_storage::{InMemoryResourceRepository, ResourceRepository};
use agentkube_tasks::AgentTask;
use assert_cmd::Command;
use std::{io::Write, sync::Arc, time::Duration};
use tempfile::NamedTempFile;
use tokio::net::TcpListener;

struct TestServer {
    base_url: String,
    tasks: Arc<InMemoryResourceRepository<AgentTask>>,
}

async fn spawn_server() -> TestServer {
    let agents = Arc::new(InMemoryResourceRepository::<AgentDefinition>::new());
    let deployments = Arc::new(InMemoryResourceRepository::<AgentDeployment>::new());
    let tasks = Arc::new(InMemoryResourceRepository::<AgentTask>::new());
    let queue = Arc::new(InMemoryTaskQueue::new());
    let state = ApiState::new(agents, deployments, tasks.clone(), queue);
    let app = router(state, 2 * 1024 * 1024);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve(listener, app).await;
    });
    // Give the listener a moment to accept connections.
    tokio::time::sleep(Duration::from_millis(50)).await;
    TestServer {
        base_url: format!("http://{addr}"),
        tasks,
    }
}

fn akctl() -> Command {
    // Keep binary tests hermetic: never hit the real update endpoint.
    // The update-check path is covered by dedicated tests below.
    let mut cmd = Command::cargo_bin("akctl").unwrap();
    cmd.env("AGENTKUBE_NO_UPDATE_CHECK", "1");
    cmd
}

const AGENT_YAML: &str = r#"
apiVersion: agentkube.ai/v1
kind: Agent
metadata:
  name: backend-agent
spec:
  role: developer
  model:
    strategy: fixed
    provider: openai
    model: gpt-5
  instructions: Build reliable software.
"#;

const DEPLOYMENT_YAML: &str = r#"
apiVersion: agentkube.ai/v1
kind: AgentDeployment
metadata:
  name: workers
spec:
  replicas: 2
  template:
    role: developer
    model:
      strategy: fixed
      provider: openai
      model: gpt-5
    instructions: Serve work.
"#;

const TASK_YAML: &str = r#"
apiVersion: agentkube.ai/v1
kind: AgentTask
metadata:
  name: review-code
spec:
  objective: Review the code.
"#;

fn write_temp(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(content.as_bytes()).unwrap();
    file
}

#[test]
fn help_and_version_succeed() {
    akctl().arg("--help").assert().success();
    akctl()
        .arg("version")
        .assert()
        .success()
        .stdout(predicates::str::contains("akctl"));
}

#[test]
fn invalid_arguments_exit_with_code_2() {
    akctl()
        .arg("get")
        .arg("starship")
        .assert()
        .failure()
        .code(2);
    akctl()
        .args(["--server", "ftp://example.com", "version"])
        .assert()
        .failure()
        .code(2);
    akctl()
        .args(["--timeout", "banana", "version"])
        .assert()
        .failure()
        .code(2);
    akctl()
        .args(["get", "agents", "--page-size", "0"])
        .assert()
        .failure()
        .code(2);
    akctl()
        .args(["get", "agents", "--page-size", "201"])
        .assert()
        .failure()
        .code(2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_flag_takes_precedence_over_environment() {
    let server = spawn_server().await;
    // Flag wins over a broken environment value.
    akctl()
        .env("AGENTKUBE_SERVER", "http://127.0.0.1:1")
        .args(["--server", &server.base_url, "health"])
        .assert()
        .success();
    // Environment is used when the flag is absent.
    akctl()
        .env("AGENTKUBE_SERVER", &server.base_url)
        .arg("health")
        .assert()
        .success();
}

#[test]
fn dry_run_accepts_json_yaml_stdin_and_multi_doc() {
    use std::io::Write as _;
    // YAML file.
    let yaml = write_temp(AGENT_YAML);
    akctl()
        .args([
            "apply",
            "-f",
            yaml.path().to_str().unwrap(),
            "--dry-run=client",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("validated"));

    // JSON by content (not extension).
    let json = serde_json::json!({
        "apiVersion": "agentkube.ai/v1",
        "kind": "AgentTask",
        "metadata": {"name": "json-task"},
        "spec": {"objective": "Do JSON work."},
    });
    let mut json_file = NamedTempFile::with_suffix(".txt").unwrap();
    json_file
        .write_all(serde_json::to_string(&json).unwrap().as_bytes())
        .unwrap();
    akctl()
        .args([
            "apply",
            "-f",
            json_file.path().to_str().unwrap(),
            "--dry-run=client",
        ])
        .assert()
        .success();

    // Multi-document stream.
    let multi = format!("{AGENT_YAML}\n---\n{TASK_YAML}\n");
    let multi_file = write_temp(&multi);
    akctl()
        .args([
            "apply",
            "-f",
            multi_file.path().to_str().unwrap(),
            "--dry-run=client",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Agent"))
        .stdout(predicates::str::contains("AgentTask"));

    // Stdin via -.
    akctl()
        .args(["apply", "-f", "-", "--dry-run=client"])
        .write_stdin(AGENT_YAML)
        .assert()
        .success();

    // Unknown kind and bad apiVersion fail with exit 2.
    let bad_kind = AGENT_YAML.replace("kind: Agent", "kind: Starship");
    let bad_file = write_temp(&bad_kind);
    akctl()
        .args([
            "apply",
            "-f",
            bad_file.path().to_str().unwrap(),
            "--dry-run=client",
        ])
        .assert()
        .failure()
        .code(2);
    let bad_version = AGENT_YAML.replace("agentkube.ai/v1", "v1");
    let bad_file = write_temp(&bad_version);
    akctl()
        .args([
            "apply",
            "-f",
            bad_file.path().to_str().unwrap(),
            "--dry-run=client",
        ])
        .assert()
        .failure()
        .code(2);
    // Empty input is rejected.
    let empty = write_temp("");
    akctl()
        .args([
            "apply",
            "-f",
            empty.path().to_str().unwrap(),
            "--dry-run=client",
        ])
        .assert()
        .failure()
        .code(2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_apply_create_update_and_get_round_trip() {
    let server = spawn_server().await;
    let file = write_temp(AGENT_YAML);

    akctl()
        .args([
            "--server",
            &server.base_url,
            "apply",
            "-f",
            file.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("created"));

    // Second apply updates.
    akctl()
        .args([
            "--server",
            &server.base_url,
            "apply",
            "-f",
            file.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("configured"));

    // Get single as JSON contains the document.
    akctl()
        .args([
            "--server",
            &server.base_url,
            "--output",
            "json",
            "get",
            "agents",
            "backend-agent",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("backend-agent"));

    // Describe defaults to YAML.
    akctl()
        .args([
            "--server",
            &server.base_url,
            "describe",
            "agent",
            "backend-agent",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("apiVersion"));

    // Table list preserves columns.
    akctl()
        .args(["--server", &server.base_url, "get", "agents"])
        .assert()
        .success()
        .stdout(predicates::str::contains("NAME"))
        .stdout(predicates::str::contains("backend-agent"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deployment_apply_scale_and_delete_flow() {
    let server = spawn_server().await;
    let file = write_temp(DEPLOYMENT_YAML);

    akctl()
        .args([
            "--server",
            &server.base_url,
            "apply",
            "-f",
            file.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    akctl()
        .args([
            "--server",
            &server.base_url,
            "scale",
            "deployment",
            "workers",
            "--replicas",
            "4",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("scaled"));

    // Out-of-range replicas fail client-side with exit 2.
    akctl()
        .args([
            "--server",
            &server.base_url,
            "scale",
            "deployment",
            "workers",
            "--replicas",
            "10001",
        ])
        .assert()
        .failure()
        .code(2);

    // Missing deployment surfaces 404 with exit 1.
    akctl()
        .args([
            "--server",
            &server.base_url,
            "scale",
            "deployment",
            "missing",
            "--replicas",
            "2",
        ])
        .assert()
        .failure()
        .code(1);

    akctl()
        .args([
            "--server",
            &server.base_url,
            "delete",
            "deployment",
            "workers",
        ])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_apply_is_queued_and_non_terminal_delete_conflicts() {
    let server = spawn_server().await;
    let file = write_temp(TASK_YAML);

    akctl()
        .args([
            "--server",
            &server.base_url,
            "apply",
            "-f",
            file.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("created"));

    akctl()
        .args([
            "--server",
            &server.base_url,
            "--output",
            "json",
            "get",
            "tasks",
            "review-code",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("QUEUED"));

    // Deleting a QUEUED task conflicts with exit 1.
    akctl()
        .args([
            "--server",
            &server.base_url,
            "delete",
            "task",
            "review-code",
        ])
        .assert()
        .failure()
        .code(1);

    // Terminal tasks can be deleted.
    let mut terminal = agentkube_tasks::AgentTask::new(
        agentkube_core::Metadata::new("done-cli").unwrap(),
        agentkube_tasks::TaskSpec::new(agentkube_tasks::Objective::new("done").unwrap()),
    );
    terminal.cancel().unwrap();
    server.tasks.create(terminal).await.unwrap();
    akctl()
        .args(["--server", &server.base_url, "delete", "task", "done-cli"])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_shows_everything_running_prettily() {
    let server = spawn_server().await;
    for manifest in [AGENT_YAML, DEPLOYMENT_YAML, TASK_YAML] {
        let file = write_temp(manifest);
        akctl()
            .args([
                "--server",
                &server.base_url,
                "apply",
                "-f",
                file.path().to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    akctl()
        .args(["--server", &server.base_url, "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Server:"))
        .stdout(predicates::str::contains(&server.base_url))
        .stdout(predicates::str::contains("AGENTS (1)"))
        .stdout(predicates::str::contains(
            "DEPLOYMENTS (1, desired 2, ready 0)",
        ))
        .stdout(predicates::str::contains("TASKS (1, QUEUED 1)"))
        .stdout(predicates::str::contains("backend-agent"));

    akctl()
        .args(["--server", &server.base_url, "-o", "json", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"summary\""))
        .stdout(predicates::str::contains("backend-agent"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_nodes_lists_worker_nodes_without_pagination() {
    let server = spawn_server().await;

    akctl()
        .args(["--server", &server.base_url, "get", "nodes"])
        .assert()
        .success()
        .stdout(predicates::str::contains("NODE"));

    akctl()
        .args(["--server", &server.base_url, "-o", "json", "get", "nodes"])
        .assert()
        .success()
        .stdout(predicates::str::contains("[]"));

    // Nodes are list-only: names and pagination are usage errors.
    akctl()
        .args(["--server", &server.base_url, "get", "nodes", "some-node"])
        .assert()
        .failure()
        .code(2);
    akctl()
        .args([
            "--server",
            &server.base_url,
            "get",
            "nodes",
            "--page-size",
            "10",
        ])
        .assert()
        .failure()
        .code(2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_doc_partial_apply_reports_exit_3_without_rollback_claim() {
    let server = spawn_server().await;
    // Second document duplicates the first task name, forcing a 409 after one success.
    let multi = format!("{AGENT_YAML}\n---\n{AGENT_YAML}\n");
    // Use tasks to force duplicate 409 deterministically: two identical tasks.
    let task = TASK_YAML;
    let dup_tasks = format!("{task}\n---\n{task}\n");
    let _ = multi;
    let file = write_temp(&dup_tasks);
    akctl()
        .args([
            "--server",
            &server.base_url,
            "apply",
            "-f",
            file.path().to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(3);
}

#[test]
fn oversized_input_is_rejected_with_exit_2() {
    let mut big = NamedTempFile::new().unwrap();
    let chunk = "x".repeat(1024);
    for _ in 0..(2 * 1024 + 10) {
        big.write_all(chunk.as_bytes()).unwrap();
    }
    akctl()
        .args([
            "apply",
            "-f",
            big.path().to_str().unwrap(),
            "--dry-run=client",
        ])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn broken_pipe_maps_to_clean_exit() {
    // Unit-level proof: broken pipes terminate cleanly with exit 0.
    let error = agentkube_cli::error::CliError::BrokenPipe;
    assert_eq!(error.exit_code(), 0);
    let io_error = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed");
    assert!(agentkube_cli::error::CliError::from_stdout_io("write", &io_error).is_broken_pipe());
}

async fn mock_update_server(tag: &'static str) -> String {
    let app = axum::Router::new().route(
        "/releases/latest",
        axum::routing::get(
            move || async move { axum::Json(serde_json::json!({ "tag_name": tag })) },
        ),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/releases/latest")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_notice_points_to_new_releases_on_stderr() {
    let url = mock_update_server("v9.9.9").await;
    let cache = tempfile::tempdir().unwrap();

    // `version` needs no API server, isolating the update-check path.
    // Re-enable checks for this command only (the helper disables them).
    Command::cargo_bin("akctl")
        .unwrap()
        .env_remove("AGENTKUBE_NO_UPDATE_CHECK")
        .env("AGENTKUBE_UPDATE_CHECK_URL", &url)
        .env("AGENTKUBE_CACHE_DIR", cache.path())
        .arg("version")
        .assert()
        .success()
        .stdout(predicates::str::contains("akctl"))
        .stderr(predicates::str::contains("v9.9.9"))
        .stderr(predicates::str::contains("brew"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_notice_stays_silent_when_disabled_or_current() {
    // Explicit opt-out: no notice even with a newer release available.
    let url = mock_update_server("v9.9.9").await;
    let cache = tempfile::tempdir().unwrap();
    let output = akctl()
        .env("AGENTKUBE_UPDATE_CHECK_URL", &url)
        .env("AGENTKUBE_CACHE_DIR", cache.path())
        .arg("version")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("new version"), "{stderr}");

    // Current release: no notice even with checks enabled.
    let current = mock_update_server(concat!("v", env!("CARGO_PKG_VERSION"))).await;
    let cache = tempfile::tempdir().unwrap();
    let output = Command::cargo_bin("akctl")
        .unwrap()
        .env_remove("AGENTKUBE_NO_UPDATE_CHECK")
        .env("AGENTKUBE_UPDATE_CHECK_URL", &current)
        .env("AGENTKUBE_CACHE_DIR", cache.path())
        .arg("version")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("new version"), "{stderr}");
}
