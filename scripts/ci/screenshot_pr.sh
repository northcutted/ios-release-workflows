#!/usr/bin/env bash
set -euo pipefail
branch="codex/screenshots-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"
git switch -c "$branch"
destination=$(python3 -c 'import json,os; print(json.load(open(os.environ["IOS_RELEASE_CONFIG"]))["screenshots_path"])')
rsync -a --delete incoming/ "$destination/"
cp build/screenshots-manifest.json "$(dirname "$destination")/manifest.json"
git add "$destination/" "$(dirname "$destination")/manifest.json"
if git diff --cached --quiet; then exit 0; fi
git -c user.name='ios-release[bot]' -c user.email='ios-release[bot]@users.noreply.github.com' commit -m 'chore(screenshots): refresh reviewed App Store captures'
# The installation token is transient and is never persisted in the checkout.
auth=$(printf 'x-access-token:%s' "$GH_TOKEN" | base64 | tr -d '\n')
echo "::add-mask::$auth"
GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.https://github.com/.extraheader GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $auth" git push origin "$branch"
cat > "$RUNNER_TEMP/screenshot-pr.md" <<'BODY'
Refresh the App Store screenshot assets from the pinned capture toolchain. All configured locale/device combinations passed the screenshot coverage and dimension checks.

Review the images before merging. This pull request does not submit an App Store release.
BODY
gh pr create --repo "$GITHUB_REPOSITORY" --base main --head "$branch" --title 'chore(screenshots): refresh App Store captures' --body-file "$RUNNER_TEMP/screenshot-pr.md"
