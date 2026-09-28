#!/usr/bin/env python3
"""Resolve a user-selected run into frozen, independently verified release inputs."""
import json
import os
from pathlib import Path
import re
import tempfile

import fetch
from configuration import CONFIG
from evidence import require, write
from verify_release import verify

ACTIONS = {'Upload to TestFlight': 'testflight', 'Prepare App Store submission': 'app-store',
           'Update store metadata': 'metadata', 'Update metadata and request review': 'metadata-review'}
TAG = r'v\d+\.\d+\.\d+(?:-build-[1-9]\d{0,3}\.[1-9]\d?)?'
PROMOTION_PATHS = {'.github/workflows/release.yml', '.github/workflows/promote.yml'}


def pages(path, key, read=fetch.api):
    result = []
    for page in range(1, 101):
        data = read(f'{path}{"&" if "?" in path else "?"}per_page=100&page={page}')
        batch = data[key]
        require(isinstance(batch, list), 'Invalid paginated response')
        result.extend(batch)
        if len(batch) < 100:
            return result
    raise ValueError('Result set too large; use an explicit recent run')


def run_id(source, read=fetch.api):
    source = source.strip()
    if re.fullmatch(r'[1-9]\d*', source):
        return source
    match = re.fullmatch(r'https://github\.com/' + re.escape(CONFIG['repository'])
                         + r'/actions/runs/([1-9]\d*)/?', source)
    if match:
        return match[1]
    # A leading # distinguishes the displayed workflow number from its API ID.
    if re.fullmatch(r'#[1-9]\d*', source):
        runs = pages(f"repos/{CONFIG['repository']}/actions/workflows/main.yml/runs?branch=main", 'workflow_runs', read)
        matches = [r for r in runs if r['run_number'] == int(source[1:])]
        require(len(matches) == 1, 'Preparation run number is missing or ambiguous; use its URL')
        return str(matches[0]['id'])
    raise ValueError('Choose an exact run URL/ID or #preparation-number from this repository')


def successful_run(identifier, paths, read=fetch.api):
    run = read(f"repos/{CONFIG['repository']}/actions/runs/{identifier}")
    require(str(run['id']) == str(identifier), 'Wrong workflow run')
    require(run.get('status') == 'completed' and run.get('conclusion') == 'success', 'Selected run has not succeeded')
    require(run.get('head_branch') == 'main' and run.get('path') in paths, 'Selected run is not an approved main workflow')
    require(run.get('event') in ('push', 'workflow_dispatch'), 'Selected run has an unexpected trigger')
    require(run.get('repository', {}).get('full_name', '').lower() == CONFIG['repository'].lower(), 'Wrong run repository')
    require(run.get('head_repository', {}).get('full_name', '').lower() == CONFIG['repository'].lower(), 'Forked run source')
    require(re.fullmatch(r'[a-f0-9]{40}', run.get('head_sha', '')), 'Invalid run source')
    comparison = read(f"repos/{CONFIG['repository']}/compare/{run['head_sha']}...main")
    require(comparison.get('status') in ('ahead', 'identical'), 'Run source is not an ancestor of protected main')
    return run


def unique_artifact(run, kind, read=fetch.api, attempt=None):
    artifacts = pages(f"repos/{CONFIG['repository']}/actions/runs/{run['id']}/artifacts", 'artifacts', read)
    pattern = rf"{kind}-{run['id']}-" + (re.escape(str(attempt)) if attempt is not None else r'[1-9]\d*')
    matching = [a for a in artifacts if re.fullmatch(pattern, a['name'])]
    require(len(matching) == 1, f'Expected one {kind} artifact; select an unambiguous run')
    artifact = matching[0]
    require(not artifact.get('expired', True), 'Selected artifact expired; do not substitute another build')
    require(str(artifact.get('workflow_run', {}).get('id')) == str(run['id']), 'Artifact belongs to another run')
    require(re.fullmatch(r'sha256:[a-f0-9]{64}', artifact.get('digest', '')), 'Artifact has no trustworthy archive digest')
    return str(artifact['id']), artifact['digest'].removeprefix('sha256:')


def resolve(action, source, metadata_commit='', read=fetch.api):
    require(action in ACTIONS, 'Unknown release action')
    operation = ACTIONS[action]
    source = source.strip()
    values = {'operation': operation, 'artifact_id': '', 'sha256': '', 'processed_artifact_id': '', 'resume_source': '',
              'processed_sha256': '', 'upload_adapter': '', 'metadata_commit': '', 'release_tag': ''}
    if re.fullmatch(TAG, source):
        require(operation != 'testflight', 'A published release is already processed; choose a preparation run for TestFlight')
        manifest = fetch.release(source, mode='observe', emit=False)
        values.update(kind='published', release_tag=source)
        if operation.startswith('metadata'):
            commit = metadata_commit or os.environ['GITHUB_SHA']
            require(re.fullmatch(r'[a-f0-9]{40}', commit), 'Metadata must name an exact reviewed commit')
            comparison = read(f"repos/{CONFIG['repository']}/compare/{commit}...main")
            require(comparison.get('status') in ('ahead', 'identical'), 'Metadata commit is not on protected main')
            values['metadata_commit'] = commit
        else:
            require(not metadata_commit, 'Metadata commit is only accepted for metadata actions')
    else:
        require(not operation.startswith('metadata'), 'Metadata actions require an immutable release tag')
        require(not metadata_commit, 'Metadata commit is only accepted for metadata actions')
        selected = successful_run(run_id(source, read), {CONFIG['source_workflow']} | PROMOTION_PATHS, read)
        processed_id = processed_sha = ''
        processed_manifest = None
        if selected['path'] in PROMOTION_PATHS:
            processed_id, processed_sha = unique_artifact(selected, 'final', read)
            # Authenticate before trusting the candidate's original run identity.
            with tempfile.TemporaryDirectory() as temporary:
                fetch.extract_artifact(processed_id, processed_sha, temporary)
                processed_manifest = verify(temporary, final=True)
            promotion = processed_manifest['promotion']
            require(str(promotion['run_id']) == str(selected['id']), 'Handoff belongs to another promotion')
            require(promotion['source_sha'] == selected['head_sha'], 'Handoff promotion source differs')
            # Multiple completed attempts are rejected above rather than guessed.
            unique_artifact(selected, 'final', read, promotion['run_attempt'])
            prepared = successful_run(processed_manifest['run_id'], {CONFIG['source_workflow']}, read)
            artifact_id, sha = unique_artifact(prepared, 'candidate', read, processed_manifest['run_attempt'])
        else:
            prepared = selected
            artifact_id, sha = unique_artifact(prepared, 'candidate', read)
        fetch.candidate(artifact_id, sha)
        manifest = json.loads(Path('release-assets/release-build-manifest.json').read_text())
        if processed_manifest:
            require(manifest['candidate_id'] == processed_manifest['candidate_id'], 'Processed handoff changes the selected candidate')
            fetch.processed(processed_id, processed_sha)
        app = json.loads(Path('release-assets/app-config.json').read_text())
        require(app['upload_adapter'] in ('transporter', 'build-uploads'), 'Unknown verified upload adapter')
        values.update(kind='candidate', artifact_id=artifact_id, sha256=sha, processed_artifact_id=processed_id,
                      processed_sha256=processed_sha, upload_adapter=app['upload_adapter'], release_tag=manifest['tag'])
        if processed_id:
            values['resume_source'] = f"https://github.com/{CONFIG['repository']}/actions/runs/{selected['id']}"
    values.update(version=manifest['version'], build_number=manifest['build_number'], source_sha=manifest['source_sha'],
                  ipa_sha256=manifest['ipa_sha256'], candidate_id=manifest['candidate_id'])
    return values


def emit(values):
    write('build/release-selection.json', values)
    if os.getenv('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
            for key, value in values.items():
                require('\n' not in str(value) and '\r' not in str(value), 'Unsafe workflow output')
                output.write(f'{key}={value}\n')
    if os.getenv('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
            summary.write(f"## Verified {values['operation']} selection\n\n"
                          f"Version **{values['version']} ({values['build_number']})**\n\n"
                          f"Source `{values['source_sha']}`\n\nIPA SHA256 `{values['ipa_sha256']}`\n\n")
            summary.write('The resolved artifact identity is fixed for all remaining jobs.\n\n')
            if values['processed_artifact_id']:
                summary.write('Reusing the signed processed handoff; no new IPA transfer.\n\n')
            if values['metadata_commit']:
                summary.write(f"Reviewed metadata commit: `{values['metadata_commit']}`\n")


if __name__ == '__main__':
    require(os.environ.get('GITHUB_REF') == 'refs/heads/main', 'Run Release from protected main')
    allowed = os.environ.get('RELEASE_DISTRIBUTION_ENABLED') == 'true'
    if os.environ['RELEASE_ACTION'] == 'Upload to TestFlight':
        allowed = allowed or os.environ.get('TESTFLIGHT_CANARY_ENABLED') == 'true'
    require(allowed, 'This release action is disabled by repository policy')
    emit(resolve(os.environ['RELEASE_ACTION'], os.environ['RELEASE_SOURCE'], os.getenv('METADATA_COMMIT', '')))
