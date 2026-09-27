<#
.SYNOPSIS
  AgentKube installer for Windows (akctl + agentkube-api).

.DESCRIPTION
  Downloads the release ZIP from GitHub Releases, verifies SHA256 when
  available, extracts it, and adds the install directory to the user PATH.

.EXAMPLE
  irm https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.ps1 | iex

.EXAMPLE
  & ./install.ps1 -Version v0.1.0 -InstallDir "$env:LocalAppData\AgentKube\bin"

.PARAMETER Version
  Release tag, e.g. v0.1.0, 0.1.0, or "latest" (default).

.PARAMETER InstallDir
  Install directory (default: $env:LocalAppData\AgentKube\bin).

.PARAMETER Only
  Install "all" (default), "akctl", or "agentkube-api".
#>
[CmdletBinding()]
param(
  [string]$Version = $env:AGENTKUBE_VERSION,
  [string]$InstallDir = $env:AGENTKUBE_INSTALL_DIR,
  [ValidateSet("all", "akctl", "agentkube-api")]
  [string]$Only = "all"
)

$ErrorActionPreference = "Stop"
$Repo = "devdanielvaldez/agentkube"

if ([string]::IsNullOrWhiteSpace($Version)) { $Version = "latest" }
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
  $InstallDir = Join-Path $env:LocalAppData "AgentKube\bin"
}

# Normalize version: allow "0.1.0" or "v0.1.0".
if ($Version -eq "latest") {
  $latestUrl = "https://api.github.com/repos/$Repo/releases/latest"
  $latest = Invoke-RestMethod -Uri $latestUrl -UseBasicParsing
  $Version = $latest.tag_name
  if ([string]::IsNullOrWhiteSpace($Version)) { throw "Could not resolve latest release." }
}
if (-not $Version.StartsWith("v")) { $Version = "v$Version" }

# Detect architecture. Releases ship x86_64 Windows; ARM64 runs emulated.
$arch = $env:PROCESSOR_ARCHITECTURE
$target = "x86_64-pc-windows-msvc"
if ($arch -eq "ARM64") {
  Write-Warning "ARM64 Windows detected: installing x64 binaries (run emulated)."
}

$asset = "agentkube-$Version-$target.zip"
$baseUrl = "https://github.com/$Repo/releases/download/$Version"
$tmp = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) ("agentkube-" + [Guid]::NewGuid().ToString("N"))) -Force
try {
  Write-Host "Installing AgentKube $Version ($target) to $InstallDir ..."
  $zipPath = Join-Path $tmp.FullName $asset
  Invoke-WebRequest -Uri "$baseUrl/$asset" -OutFile $zipPath -UseBasicParsing

  # Verify checksum when published (non-fatal if missing).
  try {
    $shaPath = "$zipPath.sha256"
    Invoke-WebRequest -Uri "$baseUrl/$asset.sha256" -OutFile $shaPath -UseBasicParsing
    $expected = ((Get-Content $shaPath -Raw) -split '\s+')[0].ToLower()
    $actual = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
      throw "SHA256 mismatch for $asset. Expected $expected, got $actual."
    }
    Write-Host "SHA256 verified."
  } catch {
    if ($_.Exception.Message -like "SHA256 mismatch*") { throw }
    Write-Warning "Checksum file not found, skipping verification."
  }

  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  $extractDir = Join-Path $tmp.FullName "extract"
  Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

  $wanted = @("akctl", "agentkube-api")
  if ($Only -ne "all") { $wanted = @($Only) }
  foreach ($bin in $wanted) {
    $src = Join-Path $extractDir "$bin.exe"
    if (-not (Test-Path $src)) { throw "Asset is missing expected binary: $bin.exe" }
    Copy-Item -Path $src -Destination (Join-Path $InstallDir "$bin.exe") -Force
  }

  # Add to user PATH if missing.
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Added $InstallDir to your user PATH. Restart the terminal to use it everywhere."
  }

  Write-Host "Installed: $($wanted -join ', ') -> $InstallDir"
  & (Join-Path $InstallDir "akctl.exe") version
} finally {
  Remove-Item -Recurse -Force $tmp.FullName -ErrorAction SilentlyContinue
}
