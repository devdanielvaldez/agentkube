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
      sha256 "ee4586f965ee96f97c1f19dc1e7f7ced5af69962920ef9416eaa9ffe1e10c3f5"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.1.0/agentkube-v0.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "24e274e45fe9d4be330d7d22cdc5c08c97a957c6a10af5aa8252bf312a94dba0"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.1.0/agentkube-v0.1.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "4387500bd8db6895099dd7ffb1f01c0edbd8d465c44f7365f812531c2990fa48"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.1.0/agentkube-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "90d869ba98c45a6c83f94bf17496299022a689c1b0692b43e8c318ff7bce50e7"
    end
  end

  def install
    bin.install "akctl", "agentkube-api"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/akctl version")
  end
end
