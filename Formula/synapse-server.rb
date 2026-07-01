# Homebrew formula for synapse-server.
#
#   brew tap zhaoxini/synapse https://github.com/zhaoxini/synapse
#   brew install synapse-server
#
# Installs both synapse-server and synapse-relay from GitHub Release binaries.

class SynapseServer < Formula
  desc "Remote mobile control for the Claude Code CLI"
  homepage "https://github.com/zhaoxini/synapse"
  version "0.2.0"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/zhaoxini/synapse/releases/download/v0.2.0/synapse-0.2.0-x86_64-apple-darwin.tar.gz"
      sha256 "b5163b2a2f2c8e74563a3716613484d3af72b7ea98b3dfce93df0bb211266432"
    end
    on_arm do
      url "https://github.com/zhaoxini/synapse/releases/download/v0.2.0/synapse-0.2.0-aarch64-apple-darwin.tar.gz"
      sha256 "4c06a213a77ff2cd704d769734e28841a363b21349764548f73def3edbaf49f4"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/zhaoxini/synapse/releases/download/v0.2.0/synapse-0.2.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "bf3b304e3e18427fc2f22944ecf65fb1cd49ddb64ee8a43a7bcf61a2013c3b49"
    end
    on_arm do
      url "https://github.com/zhaoxini/synapse/releases/download/v0.2.0/synapse-0.2.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "fe4938e7a05d6b2c565a6f93ffc837482cff384b7b86a1aa1ea71eceb41e5701"
    end
  end

  def install
    bin.install "bin/synapse-server"
    bin.install "bin/synapse-relay"
    (pkgshare/"README").install "README.md"
  end

  def caveats
    <<~EOS
      Run `synapse-server` to start the bridge on this machine.
      First launch walks through email/password sign-in.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/synapse-server --version")
  end
end
