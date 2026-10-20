# Homebrew formula for gnosis v0.8.0.
# Published to SinAnima/homebrew-tap as Formula/gnosis.rb.
class Gnosis < Formula
  desc "Gnosis — enterprise knowledge compiler (CLI + library)"
  homepage "https://github.com/SinAnima/gnosis"
  version "0.8.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-aarch64.tar.gz"
      sha256 "3ece9cc7446652fd0c7958e0b404f2bb3fc748b0285c067d46120707524caff9"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-x86_64.tar.gz"
      sha256 "f43811b0ab4a98b4e2b8a6b45227973fdbbb6d78f4daaa1906c7a33abbd0c7e6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-aarch64.tar.gz"
      sha256 "b2a85a7dc10cfd8e799d554996991e19316eebc87fa382f8ed32f54b4e41033f"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-x86_64.tar.gz"
      sha256 "c79367d7d6714f61368fd13517f212ce1b0c937e175c752e0d80291a53d00221"
    end
  end

  def install
    bin.install "gnosis"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gnosis --version")
  end
end
