# Homebrew formula for gnosis v0.5.0.
# Published to SinAnima/homebrew-tap as Formula/gnosis.rb.
class Gnosis < Formula
  desc "Gnosis — enterprise knowledge compiler (CLI + library)"
  homepage "https://github.com/SinAnima/gnosis"
  version "0.5.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-aarch64.tar.gz"
      sha256 "2c9e98073c05cb67df5c5abaaeb260c70a272c0dcc7027aad3ce5c99cf2623c1"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-x86_64.tar.gz"
      sha256 "deb9873903f69a181cc8dbf234cb82f00f3f7f417fb6a48fc752b38595489b0a"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-aarch64.tar.gz"
      sha256 "663651a672a90254f59caf07fe29aea00cfe3e27da15802df3b8f6b187282cef"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-x86_64.tar.gz"
      sha256 "68efac0603f3f639de1b5b406e0c7248e3ba097689b6120dcd321e3b2aa267cc"
    end
  end

  def install
    bin.install "gnosis"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gnosis --version")
  end
end
