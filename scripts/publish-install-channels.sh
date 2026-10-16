#!/usr/bin/env bash
# Publish the Homebrew tap, Scoop bucket, and asdf plugin for one GitHub release.
# Usage: scripts/publish-install-channels.sh v0.5.0
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:-}"
if [[ -z "${TAG}" ]]; then
  echo "usage: scripts/publish-install-channels.sh vX.Y.Z" >&2
  exit 1
fi
TAG="v${TAG#v}"
VERSION="${TAG#v}"
REPO="SinAnima/gnosis"

if [[ -n "${GITHUB_ACTIONS:-}" && -z "${GH_TOKEN:-}" ]]; then
  echo "INSTALL_REPOS_TOKEN is not set." >&2
  echo "Create a fine-grained PAT that can push to SinAnima/homebrew-tap," >&2
  echo "SinAnima/scoop-bucket, and SinAnima/asdf-gnosis, and store it as" >&2
  echo "the gnosis Actions secret INSTALL_REPOS_TOKEN." >&2
  exit 1
fi

SUMS="$(mktemp)"
trap 'rm -f "${SUMS}"' EXIT
curl -fsSL "https://github.com/${REPO}/releases/download/${TAG}/SHA256SUMS" -o "${SUMS}"

hash_for() {
  local name="$1" found
  found="$(awk -v f="${name}" '$2 == f { print $1; exit }' "${SUMS}")"
  if [[ -z "${found}" ]]; then
    echo "missing checksum for ${name}" >&2
    exit 1
  fi
  printf '%s' "${found}"
}

MACOS_ARM="$(hash_for "gnosis-${TAG}-macos-aarch64.tar.gz")"
MACOS_INTEL="$(hash_for "gnosis-${TAG}-macos-x86_64.tar.gz")"
LINUX_ARM="$(hash_for "gnosis-${TAG}-linux-aarch64.tar.gz")"
LINUX_INTEL="$(hash_for "gnosis-${TAG}-linux-x86_64.tar.gz")"
WINDOWS="$(hash_for "gnosis-${TAG}-windows-x86_64.zip")"

cat > "${ROOT}/packaging/homebrew/gnosis.rb" <<EOF
# Homebrew formula for gnosis ${TAG}.
# Published to SinAnima/homebrew-tap as Formula/gnosis.rb.
class Gnosis < Formula
  desc "Gnosis — enterprise knowledge compiler (CLI + library)"
  homepage "https://github.com/${REPO}"
  version "${VERSION}"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/${REPO}/releases/download/v#{version}/gnosis-v#{version}-macos-aarch64.tar.gz"
      sha256 "${MACOS_ARM}"
    end
    on_intel do
      url "https://github.com/${REPO}/releases/download/v#{version}/gnosis-v#{version}-macos-x86_64.tar.gz"
      sha256 "${MACOS_INTEL}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/${REPO}/releases/download/v#{version}/gnosis-v#{version}-linux-aarch64.tar.gz"
      sha256 "${LINUX_ARM}"
    end
    on_intel do
      url "https://github.com/${REPO}/releases/download/v#{version}/gnosis-v#{version}-linux-x86_64.tar.gz"
      sha256 "${LINUX_INTEL}"
    end
  end

  def install
    bin.install "gnosis"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/gnosis --version")
  end
end
EOF

/usr/bin/python3 - "${ROOT}/packaging/scoop/gnosis.json" "${VERSION}" "${WINDOWS}" <<'PY'
import json
import sys

path, version, windows_hash = sys.argv[1:]
manifest = {
    "version": version,
    "description": "Gnosis — enterprise knowledge compiler (CLI + library)",
    "homepage": "https://github.com/SinAnima/gnosis",
    "license": "MIT",
    "architecture": {
        "64bit": {
            "url": f"https://github.com/SinAnima/gnosis/releases/download/v{version}/gnosis-v{version}-windows-x86_64.zip",
            "hash": windows_hash,
        }
    },
    "bin": "gnosis.exe",
    "checkver": {"github": "https://github.com/SinAnima/gnosis"},
    "autoupdate": {
        "architecture": {
            "64bit": {
                "url": "https://github.com/SinAnima/gnosis/releases/download/v$version/gnosis-v$version-windows-x86_64.zip",
                "hash": {
                    "url": "https://github.com/SinAnima/gnosis/releases/download/v$version/SHA256SUMS",
                    "regex": "([a-fA-F0-9]{64})\\s+gnosis-v$version-windows-x86_64\\.zip",
                },
            }
        }
    },
}
with open(path, "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, indent=2)
    handle.write("\n")
PY

publish_repo() {
  local name="$1"
  local dest
  dest="$(mktemp -d)"
  if ! gh repo clone "SinAnima/${name}" "${dest}"; then
    git init -b main "${dest}"
    git -C "${dest}" remote add origin "https://github.com/SinAnima/${name}.git"
  fi
  if ! git -C "${dest}" rev-parse --verify HEAD >/dev/null 2>&1; then
    git -C "${dest}" checkout -B main
  fi
  find "${dest}" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
  mkdir -p "${dest}"
  case "${name}" in
    homebrew-tap)
      mkdir -p "${dest}/Formula"
      cp "${ROOT}/packaging/homebrew/gnosis.rb" "${dest}/Formula/gnosis.rb"
      cat > "${dest}/README.md" <<EOF
# SinAnima Homebrew tap

\`\`\`bash
brew install SinAnima/tap/gnosis
\`\`\`

The formula tracks [Gnosis releases](https://github.com/${REPO}/releases). Current formula version: ${VERSION}.
EOF
      ;;
    scoop-bucket)
      cp "${ROOT}/packaging/scoop/gnosis.json" "${dest}/gnosis.json"
      cat > "${dest}/README.md" <<EOF
# SinAnima Scoop bucket

\`\`\`powershell
scoop bucket add SinAnima https://github.com/SinAnima/scoop-bucket
scoop install gnosis
\`\`\`

The manifest tracks [Gnosis releases](https://github.com/${REPO}/releases). Current manifest version: ${VERSION}.
EOF
      ;;
    asdf-gnosis)
      mkdir -p "${dest}/bin"
      cp "${ROOT}/packaging/asdf-gnosis/bin/"* "${dest}/bin/"
      cp "${ROOT}/packaging/asdf-gnosis/.gitignore" "${dest}/.gitignore"
      cp "${ROOT}/packaging/asdf-gnosis/README.md" "${dest}/README.md"
      chmod +x "${dest}/bin/"*
      ;;
    *)
      echo "unknown repo ${name}" >&2
      exit 1
      ;;
  esac
  git -C "${dest}" add -A
  if git -C "${dest}" diff --cached --quiet; then
    echo "${name}: already at ${TAG}"
    return 0
  fi
  git -C "${dest}" commit -m "Publish gnosis ${TAG}."
  if [[ -n "${GH_TOKEN:-}" ]]; then
    git -C "${dest}" -c "http.extraheader=AUTHORIZATION: bearer ${GH_TOKEN}" push origin HEAD:main
  else
    git -C "${dest}" push origin HEAD:main
  fi
  echo "published ${name} at ${TAG}"
}

publish_repo homebrew-tap
publish_repo scoop-bucket
publish_repo asdf-gnosis
