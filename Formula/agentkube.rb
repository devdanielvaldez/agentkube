# Documentation: https://docs.brew.sh/Formula-Cookbook
# This file is updated automatically by scripts/bump-formula.sh on every
# release (see .github/workflows/release.yml). Do not edit version/sha256
# by hand; run `./scripts/bump-formula.sh vX.Y.Z` instead.
class Agentkube < Formula
  desc "Orchestration layer for autonomous AI agents"
  homepage "https://github.com/devdanielvaldez/agentkube"
  version "0.2.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.2.0/agentkube-v0.2.0-aarch64-apple-darwin.tar.gz"
      sha256 "59c01fa0a1cbdabcfef342b75ba4ba5de37f1d0845f9305d551a1a14361645ff"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.2.0/agentkube-v0.2.0-x86_64-apple-darwin.tar.gz"
      sha256 "1ee0c60d088abcc8dde33d6a297737bcb5542d25e28373ebdfb65c7b96acb73c"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.2.0/agentkube-v0.2.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "3b26c951a915edcbfd523722ba34499d79eccd4aa5b6db7fdf053535cf66cb5b"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.2.0/agentkube-v0.2.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "a77c070057f74e8a06394fad495176a54a843e30cb49a4b0341dcfb4d6f9d84c"
    end
  end

  def install
    bin.install "akctl", "agentkube-api"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/akctl version")
  end
end
