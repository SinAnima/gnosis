# Homebrew formula for gnosis v0.5.2.
# Published to SinAnima/homebrew-tap as Formula/gnosis.rb.
class Gnosis < Formula
  desc "Gnosis — enterprise knowledge compiler (CLI + library)"
  homepage "https://github.com/SinAnima/gnosis"
  version "0.5.2"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-aarch64.tar.gz"
      sha256 "2a99cf8cce0a634855ed16325e6dace26a19f18443542a4044331c232fc14d84"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-x86_64.tar.gz"
      sha256 "c4bfbf35abab11b4c1cbf79b5c7e5b36f9230b76111ca7f3e14da5006ca20b7a"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-aarch64.tar.gz"
      sha256 "fbdaa23423e3f154f7f9423bbc5ca38d74d35eaa1148ff3152a53a63aa0d391b"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-x86_64.tar.gz"
      sha256 "9ba8e7d52b2ca25500dbd4a24a19894d6d76530a5493dba59e423a984a4df437"
    end
  end

  def install
    bin.install "gnosis"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gnosis --version")
  end
end
