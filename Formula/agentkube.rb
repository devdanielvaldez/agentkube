# Documentation: https://docs.brew.sh/Formula-Cookbook
# This file is updated automatically by scripts/bump-formula.sh on every
# release (see .github/workflows/release.yml). Do not edit version/sha256
# by hand; run `./scripts/bump-formula.sh vX.Y.Z` instead.
class Agentkube < Formula
  desc "Orchestration layer for autonomous AI agents"
  homepage "https://github.com/devdanielvaldez/agentkube"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.1.0/agentkube-v0.1.0-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.1.0/agentkube-v0.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.1.0/agentkube-v0.1.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.1.0/agentkube-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "akctl", "agentkube-api"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/akctl version")
  end
end
