# Homebrew formula for gnosis v0.7.0.
# Published to SinAnima/homebrew-tap as Formula/gnosis.rb.
class Gnosis < Formula
  desc "Gnosis — enterprise knowledge compiler (CLI + library)"
  homepage "https://github.com/SinAnima/gnosis"
  version "0.7.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-aarch64.tar.gz"
      sha256 "48f44fd7b0fc163d181cba84ad1636c7200f67819810474a3a72d394e70f9ab2"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-x86_64.tar.gz"
      sha256 "b30cea550d55342fcbdcaa39f98a45319fa9b1d17a758f0fa9a09e8ce0e37cf6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-aarch64.tar.gz"
      sha256 "cbefa89b6fb0b48b93dd0aa29355c3eda3014b6009915921a27ac049dd4e19ae"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-x86_64.tar.gz"
      sha256 "3fa49238775280247b42dcb976f75101b6bc88da662472b61237b0ff4fa48e81"
    end
  end

  def install
    bin.install "gnosis"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gnosis --version")
  end
end
