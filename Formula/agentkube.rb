# Documentation: https://docs.brew.sh/Formula-Cookbook
# This file is updated automatically by scripts/bump-formula.sh on every
# release (see .github/workflows/release.yml). Do not edit version/sha256
# by hand; run `./scripts/bump-formula.sh vX.Y.Z` instead.
class Agentkube < Formula
  desc "Orchestration layer for autonomous AI agents"
  homepage "https://github.com/devdanielvaldez/agentkube"
  version "0.0.2"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.2/agentkube-v0.0.2-aarch64-apple-darwin.tar.gz"
      sha256 "a8ce383aca6fa2cd949b155de7452de2f214f8aa6afe60df7e16884dad70af27"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.2/agentkube-v0.0.2-x86_64-apple-darwin.tar.gz"
      sha256 "d95ebb5f3b2c858e1e2a157113993ce99389824de0299b4314a91ca54288da18"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.2/agentkube-v0.0.2-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "4325b0759e5dbd8e0dae66e67d6b260f6d1c9965a80ffa623ffd6397f0638419"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.2/agentkube-v0.0.2-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "87782e9977d6766f3f84b5ccb537e93148826c503ce06bccb68fa227ef1fb385"
    end
  end

  def install
    bin.install "akctl", "agentkube-api"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/akctl version")
  end
end
