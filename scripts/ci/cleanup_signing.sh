#!/usr/bin/env bash
set -uo pipefail
status=0
if [[ -f "$RUNNER_TEMP/ios-release-profile-baseline.json" ]]; then
  python3 "$IOS_RELEASE_ROOT/scripts/ci/signing_profiles.py" cleanup || status=$?
fi
if [[ -n "${SIGNING_KEYCHAIN:-}" && "$SIGNING_KEYCHAIN" == "$RUNNER_TEMP/"* ]]; then
  security delete-keychain "$SIGNING_KEYCHAIN" || true
fi
if [[ -d "$RUNNER_TEMP/ios-release-signing" ]]; then
  rm -rf "$RUNNER_TEMP/ios-release-signing" || status=$?
fi
if [[ -n "${SSH_AUTH_SOCK:-}" ]]; then
  ssh-add -D || true
fi
exit "$status"
