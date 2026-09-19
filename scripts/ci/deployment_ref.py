"""Run corrected deployment tooling from a protected tag without retagging the app."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from configuration import CONFIG, require
from fetch import api
from publish_release import gh, tag_commit


def verify_ref(release_tag, ref, source, event, read=api):
    require(re.fullmatch(r'v\d+\.\d+\.\d+', release_tag or ''), 'Invalid immutable release tag')
    if ref == 'refs/tags/' + release_tag:
        return release_tag
    require(re.fullmatch(r'[a-f0-9]{40}', source or ''), 'Invalid deployment source')
    require(ref == f'refs/tags/{release_tag}-deploy-{source}', 'Wrong protected deployment tag')
    require(event in ('create', 'workflow_dispatch'), 'Unexpected deployment recovery event')
    comparison = read(f"repos/{CONFIG['repository']}/compare/{source}...main")
    require(comparison['status'] in ('ahead', 'identical'), 'Deployment tools are not on protected main')
    return release_tag


def create(root, source, read=api):
    manifest = json.loads((Path(root) / 'release-manifest.json').read_text())
    # The ordinary release event uses the caller pinned in the candidate source.
    if manifest['source_sha'] == source:
        return
    require(re.fullmatch(r'[a-f0-9]{40}', source or ''), 'Invalid deployment source')
    tag = f"{manifest['tag']}-deploy-{source}"
    verify_ref(manifest['tag'], 'refs/tags/' + tag, source, 'create', read)
    prefix = f"repos/{CONFIG['repository']}"
    release = read(f"{prefix}/releases/tags/{manifest['tag']}")
    require(release.get('immutable') is True and not release['draft'], 'Publish immutable evidence before deployment')
    existing = subprocess.run(['gh', 'api', f'{prefix}/git/ref/tags/{tag}'], text=True, capture_output=True)
    if existing.returncode:
        require('404' in existing.stderr, 'Cannot inspect deployment tag')
        gh('api', f'{prefix}/git/refs', '-X', 'POST', '-f', 'ref=refs/tags/' + tag, '-f', 'sha=' + source)
    require(tag_commit(CONFIG['repository'], tag) == source, 'Deployment tag conflicts with reviewed tools')
    print('Protected deployment ref: ' + tag)


if __name__ == '__main__':
    if sys.argv[1] == 'create':
        create('release-assets', os.environ['GITHUB_SHA'])
    else:
        tag = os.getenv('RELEASE_TAG')
        if not tag:
            match = re.fullmatch(r'refs/tags/(v\d+\.\d+\.\d+)-deploy-[a-f0-9]{40}', os.environ['GITHUB_REF'])
            require(match, 'Unexpected deployment event')
            tag = match[1]
        verify_ref(tag, os.environ['GITHUB_REF'], os.environ['GITHUB_SHA'], os.environ['GITHUB_EVENT_NAME'])
        if os.getenv('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
                output.write('release_tag=' + tag + '\n')
