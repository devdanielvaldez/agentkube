# Documentation: https://docs.brew.sh/Formula-Cookbook
# This file is updated automatically by scripts/bump-formula.sh on every
# release (see .github/workflows/release.yml). Do not edit version/sha256
# by hand; run `./scripts/bump-formula.sh vX.Y.Z` instead.
class Agentkube < Formula
  desc "Orchestration layer for autonomous AI agents"
  homepage "https://github.com/devdanielvaldez/agentkube"
  version "0.0.3"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.3/agentkube-v0.0.3-aarch64-apple-darwin.tar.gz"
      sha256 "384c31527480184af51160eaadfd20f94b0429a01f108406e77f2d166f576684"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.3/agentkube-v0.0.3-x86_64-apple-darwin.tar.gz"
      sha256 "3f4ac4fc00b157e99ad923925152cf53946b18059716b8d547d5698f88b71334"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.3/agentkube-v0.0.3-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "9da5b6a2e06c81ed14f0359dfb6037ff3911c8f610c3aa992e286e68aa259189"
    end
    on_intel do
      url "https://github.com/devdanielvaldez/agentkube/releases/download/v0.0.3/agentkube-v0.0.3-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ab994f615762b9834fa9a9b07315c91e266814772954504e3cb67981b7fc4f95"
    end
  end

  def install
    bin.install "akctl", "agentkube-api"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/akctl version")
  end
end
