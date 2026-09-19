#!/usr/bin/env python3
"""Verify independently configured producer and consumer identities before use."""
import argparse
import json
import os
from pathlib import Path
import subprocess
from evidence import CONFIG, PRODUCER, require, validate_manifest


def native_command(root, final=False):
    workflow = 'promote' if final else 'prepare'
    filename = 'release-manifest.json' if final else 'release-build-manifest.json'
    bundle = 'release-attestation.jsonl' if final else 'build-attestation.jsonl'
    predicate = 'https://northcutted.github.io/ios-release-workflows/release/v3' if final else 'https://slsa.dev/provenance/v1'
    return ['gh', 'attestation', 'verify', str(Path(root) / filename), '--bundle', str(Path(root) / bundle),
            '--repo', CONFIG['repository'], '--signer-workflow', f'github.com/{PRODUCER}/.github/workflows/{workflow}.yml',
            '--source-ref', 'refs/heads/main', '--signer-digest', os.environ['IOS_RELEASE_REVISION'],
            '--deny-self-hosted-runners', '--predicate-type', predicate]


def provenance_identity(statement, manifest):
    predicate = statement.get('predicate', {})
    source = predicate.get('invocation', {}).get('configSource', {})
    require(source.get('uri') == f"git+https://github.com/{CONFIG['repository']}@refs/heads/main", 'Wrong SLSA source repository/ref')
    require(source.get('digest', {}).get('sha1') == manifest['source_sha'], 'Wrong SLSA source commit')
    require(source.get('entryPoint') == CONFIG['source_workflow'], 'Wrong consumer prepare entrypoint')
    environment = predicate.get('invocation', {}).get('environment', {})
    require(environment.get('github_run_id') == str(manifest['run_id']), 'Wrong producing run')
    require(int(environment.get('github_run_attempt', 0)) >= int(manifest['run_attempt']), 'Provenance predates candidate')
    number = int(environment.get('github_run_number', 0)) + CONFIG['build_number_offset']
    require(manifest['build_number'] == f"{number}.{manifest['run_attempt']}", 'Wrong producing build number')


def verify(root, source=None, tag=None, final=False, consumed=None):
    root = Path(root)
    command = native_command(root, final)
    if source and not final:
        command += ['--source-digest', source]
    subprocess.run(command, check=True)
    manifest = validate_manifest(root, 'release-manifest.json' if final else 'release-build-manifest.json', source, tag, final, consumed)
    subprocess.run(native_command(root) + ['--source-digest', manifest['source_sha']], check=True)
    if final:
        require(manifest['promotion']['revision'] == os.environ['IOS_RELEASE_REVISION'], 'Wrong promotion producer')
        subprocess.run(command + ['--source-digest', manifest['promotion']['source_sha']], check=True)
    result = subprocess.check_output(['slsa-verifier', 'verify-artifact', str(root / 'release-build-manifest.json'),
        '--provenance-path', str(root / 'provenance.intoto.jsonl'), '--source-uri', f"github.com/{CONFIG['repository']}",
        '--source-branch', 'main', '--print-provenance'], text=True)
    provenance_identity(json.loads(result), manifest)
    if consumed is None or 'application.ipa' in consumed:
        subprocess.run(['slsa-verifier', 'verify-artifact', str(root / 'application.ipa'), '--provenance-path', str(root / 'provenance.intoto.jsonl'),
                    '--source-uri', f"github.com/{CONFIG['repository']}", '--source-branch', 'main'], check=True)
    return manifest


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('root')
    parser.add_argument('--source')
    parser.add_argument('--tag')
    parser.add_argument('--final', action='store_true')
    args = parser.parse_args()
    verify(args.root, args.source, args.tag, args.final)
