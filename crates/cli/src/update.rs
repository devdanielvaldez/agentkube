//! Update notifications for `akctl`.
//!
//! After each command the CLI compares its own version against the latest
//! GitHub release and prints a notice to stderr when an upgrade exists. The
//! result is cached for [`CHECK_TTL`] so the common path never touches the
//! network, and every failure mode is silent: update checks never break, slow
//! down meaningfully, or pollute stdout for, a command.

use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    time::Duration,
};

/// How long a check result is reused without network access.
pub const CHECK_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Upper bound for the update-check request itself.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(2);
/// Setting this variable to a truthy value disables update checks.
pub const NO_UPDATE_CHECK_ENV: &str = "AGENTKUBE_NO_UPDATE_CHECK";
/// Override for the latest-release endpoint (used by tests).
pub const UPDATE_CHECK_URL_ENV: &str = "AGENTKUBE_UPDATE_CHECK_URL";
/// Override for the cache base directory (used by tests).
pub const CACHE_DIR_ENV: &str = "AGENTKUBE_CACHE_DIR";
/// Latest-release endpoint checked for new versions.
pub const DEFAULT_UPDATE_URL: &str =
    "https://api.github.com/repos/devdanielvaldez/agentkube/releases/latest";

/// Cached update-check result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct CacheState {
    checked_at: u64,
    latest: String,
}

/// Returns true unless update checks were explicitly disabled.
///
/// Any value other than unset, empty, `0`, `false`, or `no` (case
/// insensitive) disables the check, e.g. `AGENTKUBE_NO_UPDATE_CHECK=1`.
#[must_use]
pub fn update_checks_enabled() -> bool {
    match std::env::var(NO_UPDATE_CHECK_ENV) {
        Err(_) => true,
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no"
        ),
    }
}

/// Returns the notice to print when `latest` is newer than `current`.
///
/// Both values accept an optional `v` prefix and pre-release suffixes; tags
/// that cannot be parsed never produce a notice.
#[must_use]
pub fn format_notice(current: &str, latest: &str) -> String {
    let trimmed = current.trim();
    let have = if trimmed.starts_with(['v', 'V']) {
        trimmed.to_owned()
    } else {
        format!("v{trimmed}")
    };
    format!(
        "akctl: a new version {latest} is available (you have {have}).\n\
         Update with: brew reinstall https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/Formula/agentkube.rb\n\
         or: curl -fsSL https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.sh | bash -s -- {latest}"
    )
}

/// Returns true when `latest` is strictly newer than `current`.
#[must_use]
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let (mut current_core, current_pre) = match split_version(current) {
        Some(parsed) => parsed,
        None => return false,
    };
    let (mut latest_core, latest_pre) = match split_version(latest) {
        Some(parsed) => parsed,
        None => return false,
    };
    let width = current_core.len().max(latest_core.len());
    current_core.resize(width, 0);
    latest_core.resize(width, 0);
    match latest_core.cmp(&current_core) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => match (current_pre, latest_pre) {
            (None, None) => false,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(current), Some(latest)) => latest > current,
        },
    }
}

/// Checks for a newer release, honoring configuration and cache.
///
/// Returns the notice text when an upgrade exists and `None` otherwise.
/// Network, filesystem, and parsing failures are all silent.
pub async fn update_notice(current_version: &str) -> Option<String> {
    if !update_checks_enabled() {
        return None;
    }
    let url = std::env::var(UPDATE_CHECK_URL_ENV).unwrap_or_else(|_| DEFAULT_UPDATE_URL.to_owned());
    update_notice_with(
        current_version,
        default_cache_file().as_deref(),
        &url,
        CHECK_TTL,
        unix_now(),
    )
    .await
}

/// Testable core of [`update_notice`] with explicit cache, endpoint, TTL, and clock.
pub async fn update_notice_with(
    current_version: &str,
    cache_path: Option<&Path>,
    url: &str,
    ttl: Duration,
    now: u64,
) -> Option<String> {
    let cached = cache_path.and_then(read_cache);
    if let Some(state) = &cached {
        if now.saturating_sub(state.checked_at) < ttl.as_secs() {
            return is_newer_version(current_version, &state.latest)
                .then(|| format_notice(current_version, &state.latest));
        }
    }
    match fetch_latest(url, current_version).await {
        Some(latest) => {
            if let Some(path) = cache_path {
                write_cache(
                    path,
                    &CacheState {
                        checked_at: now,
                        latest: latest.clone(),
                    },
                );
            }
            is_newer_version(current_version, &latest)
                .then(|| format_notice(current_version, &latest))
        }
        // A stale cache is still informative when the network is unavailable.
        None => cached.and_then(|state| {
            is_newer_version(current_version, &state.latest)
                .then(|| format_notice(current_version, &state.latest))
        }),
    }
}

/// Resolves the cache file, honoring `AGENTKUBE_CACHE_DIR` for tests.
#[must_use]
pub fn default_cache_file() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(CACHE_DIR_ENV) {
        return Some(PathBuf::from(dir).join("akctl").join("update-check.json"));
    }
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    base.map(|dir| dir.join("akctl").join("update-check.json"))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Splits a version into numeric core components and an optional pre-release suffix.
fn split_version(input: &str) -> Option<(Vec<u64>, Option<&str>)> {
    let trimmed = input.trim();
    let without_prefix = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    if without_prefix.is_empty() {
        return None;
    }
    let (core, pre) = match without_prefix.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_prefix, None),
    };
    if core.is_empty() {
        return None;
    }
    let mut components = Vec::new();
    for part in core.split('.') {
        components.push(part.parse::<u64>().ok()?);
    }
    Some((components, pre))
}

fn read_cache(path: &Path) -> Option<CacheState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(path: &Path, state: &CacheState) {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(text) = serde_json::to_string(state) {
        let _ = std::fs::write(path, text);
    }
}

/// Fetches the latest release tag; every failure yields `None`.
async fn fetch_latest(url: &str, current_version: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .user_agent(format!("akctl/{current_version}"))
        .build()
        .ok()?;
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    body.get("tag_name")?.as_str().map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_detection_handles_prefixes_widths_and_prereleases() {
        let cases = [
            ("0.1.0", "v0.2.0", true),
            ("v0.1.0", "0.2.0", true),
            ("0.1.0", "0.1.0", false),
            ("0.2.0", "0.1.0", false),
            ("0.1.0", "0.1.1", true),
            ("0.1", "0.1.0", false),
            ("1.9.0", "1.10.0", true),
            ("0.1.0-rc.1", "0.1.0", true),
            ("0.1.0", "0.1.0-rc.1", false),
            ("0.1.0-rc.1", "0.1.0-rc.2", true),
            ("banana", "v9.9.9", false),
            ("0.1.0", "banana", false),
            ("", "v1.0.0", false),
        ];
        for (current, latest, expected) in cases {
            assert_eq!(
                is_newer_version(current, latest),
                expected,
                "{current} vs {latest}"
            );
        }
    }

    #[test]
    fn notice_mentions_both_versions_and_upgrade_paths() {
        let notice = format_notice("0.1.0", "v0.2.0");

        assert!(notice.contains("v0.2.0"), "{notice}");
        assert!(notice.contains("v0.1.0"), "{notice}");
        assert!(notice.contains("brew"), "{notice}");
        assert!(notice.contains("install.sh"), "{notice}");
    }

    #[test]
    fn cache_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");

        assert!(read_cache(&path).is_none());
        write_cache(
            &path,
            &CacheState {
                checked_at: 123,
                latest: "v9.9.9".to_owned(),
            },
        );
        assert_eq!(read_cache(&path).unwrap().latest, "v9.9.9");
    }

    async fn mock_tag_server(tag: &'static str) -> String {
        let app = axum::Router::new().route(
            "/releases/latest",
            axum::routing::get(move || async move {
                axum::Json(serde_json::json!({ "tag_name": tag }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}/releases/latest")
    }

    #[tokio::test]
    async fn fresh_cache_is_used_without_network() {
        let url = mock_tag_server("v8.8.8").await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");
        write_cache(
            &path,
            &CacheState {
                checked_at: 1_000_000,
                latest: "v9.9.9".to_owned(),
            },
        );

        let notice = update_notice_with("0.1.0", Some(&path), &url, CHECK_TTL, 1_000_060)
            .await
            .expect("fresh cache must win without network");

        assert!(notice.contains("v9.9.9"), "{notice}");
        assert!(!notice.contains("v8.8.8"), "{notice}");
    }

    #[tokio::test]
    async fn stale_cache_triggers_a_network_refresh() {
        let url = mock_tag_server("v8.8.8").await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");
        write_cache(
            &path,
            &CacheState {
                checked_at: 0,
                latest: "v0.0.1".to_owned(),
            },
        );

        let notice = update_notice_with(
            "0.1.0",
            Some(&path),
            &url,
            CHECK_TTL,
            CHECK_TTL.as_secs() + 10,
        )
        .await
        .expect("refresh must find v8.8.8");

        assert!(notice.contains("v8.8.8"), "{notice}");
        assert_eq!(read_cache(&path).unwrap().latest, "v8.8.8");
    }

    #[tokio::test]
    async fn network_failure_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");

        let notice =
            update_notice_with("0.1.0", Some(&path), "http://127.0.0.1:1/", CHECK_TTL, 0).await;

        assert!(notice.is_none());
    }
}
