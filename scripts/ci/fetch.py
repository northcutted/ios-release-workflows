#!/usr/bin/env python3
"""Fetch an explicitly identified artifact; authenticate before exposing its contents."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import zipfile
from evidence import CONFIG, digest, require
from verify_release import verify, native_command


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path], text=True))


def outputs(manifest):
    values = {k: manifest[k] for k in ('source_sha', 'version', 'build_number', 'run_id', 'run_attempt', 'tag', 'candidate_id')}
    if os.getenv('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
            for key, value in values.items():
                require('\n' not in str(value), 'Unsafe output')
                out.write(f'{key}={value}\n')
    if os.getenv('GITHUB_ENV'):
        with open(os.environ['GITHUB_ENV'], 'a') as out:
            out.write(f"IOS_RELEASE_CONFIG={Path('release-assets/app-config.json').resolve()}\n")
    return values


def extract_artifact(artifact_id, expected_digest, root):
    prefix = f"repos/{CONFIG['repository']}"
    root = Path(root); root.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        archive = Path(temp) / 'candidate.zip'
        with archive.open('wb') as output:
            subprocess.run(['gh', 'api', f'{prefix}/actions/artifacts/{artifact_id}/zip'], stdout=output, check=True)
        require(digest(archive) == expected_digest, 'Downloaded candidate checksum mismatch')
        with zipfile.ZipFile(archive) as zipped:
            names = set()
            require(sum(i.file_size for i in zipped.infolist()) <= 4 * 1024**3, 'Oversized candidate')
            for item in zipped.infolist():
                require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', item.filename) and item.filename not in names, 'Unsafe/duplicate candidate member')
                require((item.external_attr >> 16) & 0o170000 != 0o120000, 'Candidate symlink')
                names.add(item.filename)
            zipped.extractall(root)


def candidate(artifact_id, expected_digest):
    require(re.fullmatch(r'[1-9]\d*', artifact_id), 'An explicit artifact ID is required')
    require(re.fullmatch(r'[a-f0-9]{64}', expected_digest), 'An explicit artifact SHA256 is required')
    prefix = f"repos/{CONFIG['repository']}"
    artifact = api(f'{prefix}/actions/artifacts/{artifact_id}')
    require(not artifact['expired'] and artifact['digest'] == 'sha256:' + expected_digest, 'Candidate expired or digest changed')
    run = api(f"{prefix}/actions/runs/{artifact['workflow_run']['id']}")
    require(run['conclusion'] == 'success' and run['head_branch'] == 'main' and run['path'] == CONFIG['source_workflow'], 'Candidate was not prepared successfully on protected main')
    comparison = api(f"{prefix}/compare/{run['head_sha']}...main")
    require(comparison['status'] in ('ahead', 'identical'), 'Candidate is not an ancestor of protected main')
    root = Path('release-assets')
    extract_artifact(artifact_id, expected_digest, root)
    manifest = verify(root, source=run['head_sha'])
    require(str(manifest['run_id']) == str(run['id']), 'Candidate belongs to another run')
    require(artifact['name'] == f"candidate-{manifest['run_id']}-{manifest['run_attempt']}", 'Wrong candidate artifact')
    result = subprocess.run(['gh', 'api', f"{prefix}/releases/tags/{manifest['tag']}"], text=True, capture_output=True)
    published = False
    if result.returncode == 0 and not json.loads(result.stdout)['draft']:
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as temporary:
            try:
                os.chdir(temporary)
                existing = release(manifest['tag'], mode='observe', emit=False)
                require(existing['candidate_id'] == manifest['candidate_id'], 'Version already published from a different candidate')
                published = True
            finally:
                os.chdir(previous)
    elif result.returncode != 0:
        require('404' in result.stderr, 'Unable to check existing release')
    if os.getenv('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
            out.write(f"already_published={str(published).lower()}\n")
    return outputs(manifest)


def processed(artifact_id, expected_digest):
    """An explicit, signed canary handoff can be promoted without another transfer."""
    require(re.fullmatch(r'[1-9]\d*', artifact_id), 'An explicit processed artifact ID is required')
    require(re.fullmatch(r'[a-f0-9]{64}', expected_digest), 'An explicit processed SHA256 is required')
    candidate_manifest = verify('release-assets')
    artifact = api(f"repos/{CONFIG['repository']}/actions/artifacts/{artifact_id}")
    require(not artifact['expired'] and artifact['digest'] == 'sha256:' + expected_digest, 'Processed artifact expired or changed')
    with tempfile.TemporaryDirectory() as temp:
        extract_artifact(artifact_id, expected_digest, temp)
        manifest = verify(temp, source=candidate_manifest['source_sha'], final=True)
        require(manifest['candidate_id'] == candidate_manifest['candidate_id'], 'Processed handoff belongs to another candidate')
        promotion = manifest['promotion']
        require(str(artifact['workflow_run']['id']) == str(promotion['run_id']), 'Wrong promotion run')
        require(artifact['name'] == f"final-{promotion['run_id']}-{promotion['run_attempt']}", 'Wrong processed artifact')
        # Only the signed processing receipt is needed; never execute artifact code.
        import shutil
        shutil.copyfile(Path(temp) / 'testflight-status.json', 'release-assets/testflight-status.json')
    print('Authenticated processed candidate; no new upload required')


def release(tag, mode="deploy", emit=True):
    require(re.fullmatch(r'v\d+\.\d+\.\d+', tag), 'Invalid release tag')
    release = api(f"repos/{CONFIG['repository']}/releases/tags/{tag}")
    require(release.get('immutable') is True and not release['draft'] and not release['prerelease'], 'Expected published immutable release')
    Path('release-assets').mkdir(exist_ok=True)
    command = ['gh', 'release', 'download', tag, '--repo', CONFIG['repository'], '--dir', 'release-assets']
    subprocess.run(command + ['-p', 'release-manifest.json', '-p', 'release-attestation.jsonl'], check=True)
    subprocess.run(native_command('release-assets', final=True), check=True)
    signed = json.loads(Path('release-assets/release-manifest.json').read_text())
    names = {a['name'] for a in signed['artifacts']}
    consumed = {'release-build-manifest.json', 'build-attestation.jsonl', 'provenance.intoto.jsonl', 'qa-manifest.json', 'app-config.json', 'testflight-status.json'}
    if mode == 'deploy':
        consumed |= {'app-store-metadata.tar.zst', 'app-store-screenshots.tar.zst', 'screenshots-manifest.json'}
        if 'accessibility.json' in names: consumed.add('accessibility.json')
    require(consumed <= names, 'Incomplete signed release evidence')
    subprocess.run(command + [arg for name in sorted(consumed) for arg in ['-p', name]], check=True)
    manifest = verify('release-assets', tag=tag, final=True, consumed=consumed)
    from publish_release import tag_commit
    require(tag_commit(CONFIG['repository'], tag) == manifest['source_sha'], 'Release tag points to another commit')
    return outputs(manifest) if emit else manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('kind', choices=['candidate', 'processed', 'release'])
    args = parser.parse_args()
    if args.kind == 'candidate':
        candidate(os.environ['CANDIDATE_ARTIFACT_ID'], os.environ['CANDIDATE_SHA256'])
    elif args.kind == 'processed':
        processed(os.environ['PROCESSED_ARTIFACT_ID'], os.environ['PROCESSED_SHA256'])
    else:
        release(os.environ['RELEASE_TAG'])
