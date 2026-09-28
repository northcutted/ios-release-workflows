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

TAG_PATTERN = r'v\d+\.\d+\.\d+(?:-build-[1-9]\d{0,3}\.[1-9]\d?)?'


def operation_ref(ref):
    """Operation parameters are immutable parts of a publisher-only v* tag."""
    match = re.fullmatch(r'refs/tags/(' + TAG_PATTERN + r')-op-([1-9]\d*)-(stage|submit)-(original|[a-f0-9]{40})-deploy-([a-f0-9]{40})', ref)
    if not match:
        return None
    tag, run, mode, metadata, source = match.groups()
    return {'release_tag': tag, 'run_id': run, 'submit': mode == 'submit',
            'metadata_commit': '' if metadata == 'original' else metadata, 'source_sha': source}


def verify_ref(release_tag, ref, source, event, read=api):
    require(re.fullmatch(TAG_PATTERN, release_tag or ''), 'Invalid immutable release tag')
    if ref == 'refs/tags/' + release_tag:
        return release_tag
    require(re.fullmatch(r'[a-f0-9]{40}', source or ''), 'Invalid deployment source')
    operation = operation_ref(ref)
    if operation:
        require(operation['release_tag'] == release_tag and operation['source_sha'] == source, 'Wrong protected operation identity')
        if operation['metadata_commit']:
            require(read(f"repos/{CONFIG['repository']}/compare/{operation['metadata_commit']}...main")['status'] in ('ahead', 'identical'), 'Operation metadata is not on protected main')
    else:
        require(ref == f'refs/tags/{release_tag}-deploy-{source}', 'Wrong protected deployment tag')
    require(event in ('create', 'workflow_dispatch'), 'Unexpected deployment recovery event')
    comparison = read(f"repos/{CONFIG['repository']}/compare/{source}...main")
    require(comparison['status'] in ('ahead', 'identical'), 'Deployment tools are not on protected main')
    return release_tag


def request(root, source, run_id, metadata_commit, submit, read=api):
    manifest = json.loads((Path(root) / 'release-manifest.json').read_text())
    require(re.fullmatch(r'[1-9]\d*', run_id), 'Invalid operation run')
    require(not metadata_commit or re.fullmatch(r'[a-f0-9]{40}', metadata_commit), 'Metadata commit must be exact')
    require(isinstance(submit, bool), 'Explicit submission choice required')
    tag = f"{manifest['tag']}-op-{run_id}-{'submit' if submit else 'stage'}-{metadata_commit or 'original'}-deploy-{source}"
    verify_ref(manifest['tag'], 'refs/tags/' + tag, source, 'create', read)
    prefix = f"repos/{CONFIG['repository']}"
    release = read(f"{prefix}/releases/tags/{manifest['tag']}")
    require(release.get('immutable') is True and not release['draft'], 'Operation requires an immutable release')
    existing = subprocess.run(['gh', 'api', f'{prefix}/git/ref/tags/{tag}'], text=True, capture_output=True)
    if existing.returncode:
        require('404' in existing.stderr, 'Cannot inspect operation tag')
        gh('api', f'{prefix}/git/refs', '-X', 'POST', '-f', 'ref=refs/tags/' + tag, '-f', 'sha=' + source)
    require(tag_commit(CONFIG['repository'], tag) == source, 'Operation tag conflicts with reviewed tools')
    # The create event is sent only once. A retry reuses the same tag instead of
    # dispatching a duplicate Apple operation. Failed deployment jobs are retried
    # in their original run so their operation receipts remain available.
    if os.getenv('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as out:
            out.write(f"Protected deployment operation: `{tag}`. Its App Store Deploy run handles staging and any requested production approval. If it failed, re-run that run's failed jobs.\n")
    return tag


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
    elif sys.argv[1] == 'request':
        require(os.environ.get('GITHUB_REF') == 'refs/heads/main', 'Request deployment from protected main')
        require(os.environ.get('REQUEST_SUBMISSION') in ('true', 'false'), 'Explicit submission choice required')
        manifest = json.loads(Path('release-assets/release-manifest.json').read_text())
        require(manifest['tag'] == os.environ['RELEASE_TAG'], 'Selected release differs')
        request('release-assets', os.environ['GITHUB_SHA'], os.environ['GITHUB_RUN_ID'],
                os.getenv('METADATA_COMMIT', ''), os.environ['REQUEST_SUBMISSION'] == 'true')
    else:
        tag = os.getenv('RELEASE_TAG')
        operation = operation_ref(os.environ['GITHUB_REF'])
        if not tag:
            if operation:
                tag = operation['release_tag']
            else:
                match = re.fullmatch(r'refs/tags/(' + TAG_PATTERN + r')-deploy-[a-f0-9]{40}', os.environ['GITHUB_REF'])
                require(match, 'Unexpected deployment event')
                tag = match[1]
        verify_ref(tag, os.environ['GITHUB_REF'], os.environ['GITHUB_SHA'], os.environ['GITHUB_EVENT_NAME'])
        metadata = os.getenv('METADATA_COMMIT', '')
        submit = os.getenv('REQUEST_SUBMISSION', 'true')
        require(submit in ('true', 'false'), 'Invalid submission choice')
        if operation:
            require(not metadata or metadata == operation['metadata_commit'], 'Metadata cannot override the immutable operation')
            metadata = operation['metadata_commit']
            submit = str(operation['submit']).lower()
        require(not metadata or re.fullmatch(r'[a-f0-9]{40}', metadata), 'Metadata commit must be exact')
        if os.getenv('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
                output.write('release_tag=' + tag + '\n')
                output.write('metadata_commit=' + metadata + '\n')
                output.write('submit=' + submit + '\n')
