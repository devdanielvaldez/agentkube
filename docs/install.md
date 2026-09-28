# Install AgentKube

This guide covers installing `akctl` and `agentkube-api` on **macOS**,
**Linux**, and **Windows** from prebuilt GitHub Releases, plus building from
source with Cargo.

> Latest releases: `https://github.com/devdanielvaldez/agentkube/releases`
> Requires nothing else at runtime (TLS uses bundled Rustls, no OpenSSL).

## Homebrew (macOS and Linux)

```bash
brew tap devdanielvaldez/agentkube https://github.com/devdanielvaldez/agentkube
brew install agentkube
```

(Homebrew 7 no longer installs formulae from raw URLs, so the explicit tap
step is required. The two-argument form works with any repository name.)

This installs both `akctl` and `agentkube-api` from the prebuilt bottles for
your platform (macOS arm64/x86_64, Linux x86_64/arm64). The formula is bumped
automatically on every release, so upgrading is:

```bash
brew update
brew upgrade agentkube
```

## Update notifications

`akctl` checks the latest GitHub release once a day and prints a notice to
stderr when an upgrade exists. The result is cached under `~/.cache/akctl`
(`%LOCALAPPDATA%\akctl` on Windows), the check never takes more than ~2
seconds, and it fails silently when offline — commands and their JSON/YAML
stdout are never affected.

Disable it:

```bash
export AGENTKUBE_NO_UPDATE_CHECK=1
```

## Quick install (script)

### macOS (Apple Silicon and Intel)

```bash
curl -fsSL https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.sh | bash
```

Pin a version or directory:

```bash
curl -fsSL .../install.sh | bash -s -- v0.1.0
curl -fsSL .../install.sh | bash -s -- --to ~/.local/bin --only akctl
```

If `~/.local/bin` was used, add it to your shell:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

### Linux (x86_64 and arm64)

```bash
curl -fsSL https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.sh | bash
```

Same options as macOS (`VERSION`, `--to`, `--only`). The script verifies the
published SHA256 checksum when `sha256sum` or `shasum` is available.

### Windows (x64, PowerShell)

```powershell
irm https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.ps1 | iex
```

Pin a version or directory:

```powershell
& ./install.ps1 -Version v0.1.0
& ./install.ps1 -InstallDir "$env:LocalAppData\AgentKube\bin" -Only akctl
```

The installer adds the directory to your **user PATH**. Restart the terminal,
then run `akctl version`. (On ARM64 Windows the x64 binaries run emulated.)

## Verify

```bash
akctl version
akctl --help
```

Start the server in another terminal, then check health:

```bash
agentkube-api &
akctl health
```

## Operator (durable execution)

`agentkube-api` above is in-memory and idle: applied tasks stay `QUEUED`.
`agentkube-operator` is the same HTTP API on SQLite storage with reconcile,
dispatch, and embedded model execution (currently source-built only):

```bash
cargo build --release -p agentkube-operator
export AGENTKUBE_STORAGE__DATA_DIR="$HOME/.agentkube"
export AGENTKUBE_PROVIDERS__OLLAMA_BASE_URL="http://127.0.0.1:11434"
./target/release/agentkube-operator &
akctl apply -f task.yaml   # reaches a terminal state via local Ollama
akctl get nodes              # live worker heartbeats
curl -s http://127.0.0.1:8080/metrics | head
```

Relevant knobs: `AGENTKUBE_OPERATOR__RECONCILE_INTERVAL` /
`AGENTKUBE_OPERATOR__DISPATCH_INTERVAL`, `AGENTKUBE_WORKER__LEASE_TIMEOUT`
(keep above model latency), `AGENTKUBE_AUTH__TOKEN` (enforces Bearer auth on
`/v1/*`; set `AGENTKUBE_TOKEN` to the same value for `akctl`), and
`AGENTKUBE_PROVIDERS__OPENAI_API_KEY` for hosted inference. Model catalogs
for explicit specs live in `providers.ollamaModels` / `openaiModels` via a
JSON config file; see `docs/parity-plan.md`.

## Manual download

Pick the asset matching your platform from the release page:

| OS | Arch | Asset |
|---|---|---|
| macOS | Apple Silicon (arm64) | `agentkube-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS | Intel (x86_64) | `agentkube-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Linux | x86_64 | `agentkube-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| Linux | arm64 | `agentkube-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz` |
| Windows | x64 | `agentkube-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

Each archive contains `akctl` (+ `.exe` on Windows) and `agentkube-api`,
plus `CHECKSUMS.txt` alongside per-file `.sha256` files in the release.

Unix example:

```bash
VERSION=v0.1.0
TARGET=aarch64-apple-darwin  # adjust to your platform
curl -fSLO "https://github.com/devdanielvaldez/agentkube/releases/download/${VERSION}/agentkube-${VERSION}-${TARGET}.tar.gz"
curl -fSLO "https://github.com/devdanielvaldez/agentkube/releases/download/${VERSION}/agentkube-${VERSION}-${TARGET}.tar.gz.sha256"
shasum -a 256 -c "agentkube-${VERSION}-${TARGET}.tar.gz.sha256"
tar -xzf "agentkube-${VERSION}-${TARGET}.tar.gz"
install -m 755 akctl agentkube-api /usr/local/bin/
```

Windows example (PowerShell):

```powershell
$VERSION="v0.1.0"
Invoke-WebRequest "https://github.com/devdanielvaldez/agentkube/releases/download/$VERSION/agentkube-$VERSION-x86_64-pc-windows-msvc.zip" -OutFile agentkube.zip
Expand-Archive agentkube.zip -DestinationPath "$env:LocalAppData\AgentKube\bin" -Force
```

## Build from source

Requires Rust 1.88+.

```bash
git clone https://github.com/devdanielvaldez/agentkube.git
cd agentkube
cargo build --release -p agentkube-cli -p agentkube-api
./target/release/akctl version
./target/release/agentkube-api
```

Install into Cargo's bin dir:

```bash
cargo install --path crates/cli --bin akctl
cargo install --path crates/api --bin agentkube-api
```

## Upgrade and uninstall

```bash
# Upgrade (same installer, optional version):
curl -fsSL .../install.sh | bash -s -- vX.Y.Z

# Upgrade (Homebrew):
brew update
brew upgrade agentkube

# Uninstall (Unix script installs):
rm -f /usr/local/bin/akctl /usr/local/bin/agentkube-api ~/.local/bin/akctl ~/.local/bin/agentkube-api
```

```powershell
# Uninstall (Homebrew):
brew uninstall agentkube
```

```powershell
# Uninstall (Windows):
Remove-Item "$env:LocalAppData\AgentKube\bin\akctl.exe","$env:LocalAppData\AgentKube\bin\agentkube-api.exe" -ErrorAction SilentlyContinue
```

## Troubleshooting

- `command not found: akctl` → the install dir is not on `PATH`. Reopen the
  terminal or export it (see above).
- `unsupported OS/architecture` → download the manual asset for your platform.
- Corporate proxy / TLS errors → ensure `https://github.com` is reachable;
  on Windows, run PowerShell with TLS 1.2+ (default on Win10/11).
- `permission denied` on Unix → rerun with a writable `--to` dir such as
  `~/.local/bin`, or install `sudo` for system-wide install.

## Creating a release (maintainers)

```bash
git tag v0.1.0
git push origin v0.1.0
```

Pushing a `v*` tag triggers `.github/workflows/release.yml`, which builds all
platforms, creates the GitHub Release, and uploads the `tar.gz`/`zip`
installers plus checksums. See also `scripts/release-tag.sh`.
