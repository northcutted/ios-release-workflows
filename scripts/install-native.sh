#!/usr/bin/env bash
set -euo pipefail
version="${1:-0.2.0-beta.1}"
prefix="${2:-${HOME}/.local}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-z0-9.]+)?$ ]] || { echo 'Pass a published native CLI version.' >&2; exit 1; }
command -v gh >/dev/null || { echo 'Install GitHub CLI (brew install gh), then run gh auth login.' >&2; exit 1; }
case "$(uname -s):$(uname -m)" in
  Darwin:arm64) asset=macos-arm64 ;;
  Darwin:x86_64) asset=macos-x86_64 ;;
  Linux:x86_64) asset=linux-x86_64 ;;
  *) echo 'Supported binaries: macOS arm64/x86_64 and Linux x86_64.' >&2; exit 1 ;;
esac
task_temp="$(mktemp -d)"
trap 'rm -rf "$task_temp"' EXIT
repo=northcutted/ios-release-workflows
tag="native-v${version}"
binary="ios-release-${asset}"
source="$(gh api --hostname github.com "repos/$repo/git/ref/tags/$tag" --jq 'select(.object.type == "commit") | .object.sha')"
[[ "$source" =~ ^[a-f0-9]{40}$ ]] || { echo 'Release tag must identify an exact source commit.' >&2; exit 1; }
GH_HOST=github.com gh release download "$tag" --repo "$repo" --pattern "$binary" --pattern SHA256SUMS --dir "$task_temp"
expected="$(awk -v file="$binary" '$2 == file {print $1}' "$task_temp/SHA256SUMS")"
[[ "$expected" =~ ^[a-f0-9]{64}$ ]] || { echo 'Published checksum is missing or ambiguous.' >&2; exit 1; }
if command -v sha256sum >/dev/null; then actual="$(sha256sum "$task_temp/$binary" | awk '{print $1}')"; else actual="$(shasum -a 256 "$task_temp/$binary" | awk '{print $1}')"; fi
test "$actual" = "$expected"
GH_HOST=github.com gh attestation verify "$task_temp/$binary" --repo "$repo" \
  --signer-workflow "$repo/.github/workflows/native-binaries.yml" \
  --signer-digest "$source" --source-digest "$source" \
  --source-ref refs/heads/main --deny-self-hosted-runners
mkdir -p "$prefix/bin"
if test -e "$prefix/bin/ios-release"; then
  echo "An ios-release command already exists in $prefix/bin. Choose a separate install prefix." >&2
  exit 1
fi
install -m 755 "$task_temp/$binary" "$prefix/bin/ios-release"
printf 'Installed verified ios-release %s at %s/bin/ios-release\n' "$version" "$prefix"
"$prefix/bin/ios-release" doctor
