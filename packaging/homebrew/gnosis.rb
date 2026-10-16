# Homebrew formula for gnosis v0.6.0.
# Published to SinAnima/homebrew-tap as Formula/gnosis.rb.
class Gnosis < Formula
  desc "Gnosis — enterprise knowledge compiler (CLI + library)"
  homepage "https://github.com/SinAnima/gnosis"
  version "0.6.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-aarch64.tar.gz"
      sha256 "0975eb1ebb9a5c262683d108f47b2ca37dde71043f98ae9fb6af0bac0d7299c3"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-macos-x86_64.tar.gz"
      sha256 "3218a81bea2aa30ac65cf950b42acab8c0c9b484720da55ac5f776d89c701ab9"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-aarch64.tar.gz"
      sha256 "aeeb915e5161e2def4a71bb2acae4a05b07af6b3d9f743bb2c3e88e018533b9d"
    end
    on_intel do
      url "https://github.com/SinAnima/gnosis/releases/download/v#{version}/gnosis-v#{version}-linux-x86_64.tar.gz"
      sha256 "41e009399ddac4b490e54a1bbb12bae6a5b083e9dc617094af2a61de88b81891"
    end
  end

  def install
    bin.install "gnosis"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gnosis --version")
  end
end
