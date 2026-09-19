import os
import json
from pathlib import Path
import re
import sys

root = Path(__file__).resolve().parents[2]
workspace = Path(os.environ['GITHUB_WORKSPACE']).resolve()
config = (workspace / os.environ['CONFIG_PATH']).resolve()
assert workspace in config.parents and config.is_file(), 'Configuration escapes application workspace'
revision = os.environ['PLATFORM_REVISION']
assert re.fullmatch(r'[a-f0-9]{40}', revision), 'Pin the workflow and platform_revision to the same full commit'
values = {
    'IOS_RELEASE_ROOT': str(root), 'IOS_APP_ROOT': str(workspace), 'IOS_RELEASE_CONFIG': str(config),
    'IOS_RELEASE_REVISION': revision, 'BUNDLE_GEMFILE': str(root / 'Gemfile'),
    'BUNDLE_PATH': str(Path(os.environ['RUNNER_TEMP']) / 'ios-release-gems'),
    'BUNDLE_APP_CONFIG': str(Path(os.environ['RUNNER_TEMP']) / 'ios-release-bundle'),
    'BUNDLE_FROZEN': 'true', 'FASTLANE_SKIP_UPDATE_CHECK': 'true', 'FASTLANE_OPT_OUT_USAGE': 'true'
}
os.environ.update(values)
sys.path.insert(0, str(root / 'scripts/ci'))
from configuration import CONFIG
# This policy comes from the protected checkout, before a verified artifact can
# replace IOS_RELEASE_CONFIG. Artifact-supplied allowlists never establish trust.
approved = CONFIG.get('trusted_producer_revisions', [])
assert isinstance(approved, list) and len(approved) <= 20 and all(
    isinstance(value, str) and re.fullmatch(r'[a-f0-9]{40}', value) for value in approved
), 'Trusted producer revisions must be explicit full commits'
values['IOS_RELEASE_TRUSTED_PRODUCER_REVISIONS'] = json.dumps(sorted(set([revision] + approved)))
with open(os.environ['GITHUB_ENV'], 'a') as out:
    for key, value in values.items():
        assert '\n' not in value and '\r' not in value
        out.write(f'{key}={value}\n')
