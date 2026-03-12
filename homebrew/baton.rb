class Baton < Formula
  desc "A composable validation gate for AI agent outputs"
  homepage "https://github.com/apierron/baton"
  version "0.4.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/apierron/baton/releases/download/v#{version}/baton-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    end
    on_intel do
      url "https://github.com/apierron/baton/releases/download/v#{version}/baton-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/apierron/baton/releases/download/v#{version}/baton-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    end
    on_intel do
      url "https://github.com/apierron/baton/releases/download/v#{version}/baton-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "baton"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/baton version")
  end
end
