# Homebrew Formula for gnosis
#
# Copy to your tap as Formula/gnosis.rb and fill sha256 from Release SHA256SUMS.
class Gnosis < Formula
  desc "Gnosis — enterprise knowledge compiler (CLI + library)"
  homepage "https://github.com/SinAnima/gnosis"
  version "0.2.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-aarch64.tar.gz"
      # sha256 "REPLACE_AFTER_RELEASE"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-x86_64.tar.gz"
      # sha256 "REPLACE_AFTER_RELEASE"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-aarch64.tar.gz"
      # sha256 "REPLACE_AFTER_RELEASE"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-x86_64.tar.gz"
      # sha256 "REPLACE_AFTER_RELEASE"
    end
  end

  def install
    bin.install "gnosis"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gnosis --version")
  end
end
