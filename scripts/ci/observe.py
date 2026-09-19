"""Observe active releases using authenticated manifests and read-only Apple requests."""
import json
import os
from pathlib import Path
import shutil
import subprocess
from fetch import api, release
from evidence import write

root = Path('build/observations'); root.mkdir(parents=True, exist_ok=True)
releases = api(f"repos/{os.environ['GITHUB_REPOSITORY']}/releases?per_page=10")
observed = []
original_config = os.environ['IOS_RELEASE_CONFIG']
for item in releases:
    if not item.get('immutable') or item['draft'] or item['prerelease']:
        continue
    if not any(a['name'] == 'release-manifest.json' for a in item['assets']):
        continue
    shutil.rmtree('release-assets', ignore_errors=True)
    release(item['tag_name'], mode='observe')
    os.environ['IOS_RELEASE_CONFIG'] = str(Path('release-assets/app-config.json').resolve())
    os.environ['RELEASE_MANIFEST'] = str(Path('release-assets/release-manifest.json').resolve())
    path = (root / (item['tag_name'] + '.json')).resolve()
    os.environ['OBSERVATION_RECEIPT'] = str(path)
    subprocess.run(['bundle', 'exec', 'ruby', os.environ['IOS_RELEASE_ROOT'] + '/scripts/fastlane.rb', 'observe'], check=True)
    record = json.loads(path.read_text()); observed.append(record)
    os.environ['IOS_RELEASE_CONFIG'] = original_config
write(root / 'summary.json', {'releases': observed})
with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as out:
    for record in observed:
        attrs = record['version']['attributes']
        out.write(f"{attrs.get('versionString')}: {attrs.get('appVersionState') or attrs.get('appStoreState')}; phased release: {record['phased_release']}\n")
