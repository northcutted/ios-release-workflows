"""Read Apple state and report changes; old snapshots never authorize releases."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from fetch import api, release, extract_artifact
from evidence import write


def state(record):
    attrs = (record.get('version') or {}).get('attributes', {})
    return attrs.get('appVersionState') or attrs.get('appStoreState')


def active(record):
    phased = (record.get('phased_release') or {}).get('attributes', {}).get('phasedReleaseState')
    return state(record) not in ('READY_FOR_DISTRIBUTION', 'READY_FOR_SALE', 'REPLACED_WITH_NEW_VERSION') or phased not in (None, 'COMPLETE')


def meaningful(record):
    build = record.get('build') or {}
    version = record.get('version') or {}
    attributes = version.get('attributes', {})
    return {'candidate_id': record.get('candidate_id'), 'build_id': build.get('id'),
            'processing': build.get('attributes', {}).get('processingState'), 'version_id': version.get('id'),
            'version': attributes.get('versionString'), 'state': state(record),
            'release_type': attributes.get('releaseType'), 'phased_release': record.get('phased_release')}


def previous(read=api):
    """Optional polling/display cache. Cache failure triggers a complete refresh."""
    repository = os.environ['GITHUB_REPOSITORY']
    workflow = os.getenv('GITHUB_WORKFLOW_REF', '').split('@')[0].rsplit('/', 1)[-1]
    if not re.fullmatch(r'[A-Za-z0-9_-]+\.ya?ml', workflow):
        return {}
    prefix = f'repos/{repository}'
    try:
        runs = read(f'{prefix}/actions/workflows/{workflow}/runs?branch=main&status=success&per_page=20')['workflow_runs']
        for run in runs:
            if str(run['id']) == os.environ['GITHUB_RUN_ID'] or run.get('head_branch') != 'main' or run.get('conclusion') != 'success':
                continue
            artifacts = read(f"{prefix}/actions/runs/{run['id']}/artifacts?per_page=100")['artifacts']
            candidates = [(int(match[1]), item) for item in artifacts
                          if (match := re.fullmatch(rf"observations-{run['id']}-([1-9]\d*)", item['name']))
                          and not item['expired'] and item['size_in_bytes'] < 1024 * 1024]
            if not candidates:
                continue
            _, artifact = max(candidates, key=lambda entry: entry[0])
            if not re.fullmatch(r'sha256:[a-f0-9]{64}', artifact.get('digest', '')):
                continue
            with tempfile.TemporaryDirectory() as temp:
                extract_artifact(str(artifact['id']), artifact['digest'][7:], temp)
                data = json.loads((Path(temp) / 'summary.json').read_text())
            return {item['release_tag']: item for item in data['releases'] if 'release_tag' in item}
    except (KeyError, ValueError, OSError, subprocess.SubprocessError) as error:
        print(f'Previous observation unavailable; refreshing every release ({type(error).__name__}).')
    return {}


def eligible(releases):
    return [item for item in releases if item.get('immutable') and not item['draft'] and not item['prerelease']
            and any(asset['name'] == 'release-manifest.json' for asset in item['assets'])]


def main():
    root = Path('build/observations'); root.mkdir(parents=True, exist_ok=True)
    releases = eligible(api(f"repos/{os.environ['GITHUB_REPOSITORY']}/releases?per_page=10"))
    baseline = previous()
    records, changed, unchanged, skipped = [], [], [], []
    original_config = os.environ['IOS_RELEASE_CONFIG']
    refresh_all = os.getenv('GITHUB_EVENT_NAME') == 'workflow_dispatch'
    for index, item in enumerate(releases):
        tag = item['tag_name']
        old = baseline.get(tag)
        # Always refresh the newest release. Unknown and incomplete states stay
        # active; a manual refresh also checks older completed releases.
        if not refresh_all and index > 0 and old and not active(old):
            records.append(old); skipped.append(tag)
            continue
        shutil.rmtree('release-assets', ignore_errors=True)
        release(tag, mode='observe')
        os.environ['IOS_RELEASE_CONFIG'] = str(Path('release-assets/app-config.json').resolve())
        os.environ['RELEASE_MANIFEST'] = str(Path('release-assets/release-manifest.json').resolve())
        path = (root / (tag + '.json')).resolve()
        os.environ['OBSERVATION_RECEIPT'] = str(path)
        try:
            subprocess.run(['bundle', 'exec', 'ruby', os.environ['IOS_RELEASE_ROOT'] + '/scripts/fastlane.rb', 'observe'], check=True)
        finally:
            os.environ['IOS_RELEASE_CONFIG'] = original_config
        record = json.loads(path.read_text())
        record['release_tag'] = tag
        write(path, record)
        records.append(record)
        if old and meaningful(old) == meaningful(record):
            unchanged.append(tag)
        else:
            changed.append({'release_tag': tag, 'before': meaningful(old) if old else None, 'after': meaningful(record)})
    write(root / 'summary.json', {'releases': records, 'changes': changed, 'unchanged': unchanged, 'retained_completed': skipped})
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as out:
        if changed:
            out.write('## App Store changes\n\n')
            for item in changed:
                before = item['before']['state'] if item['before'] else 'first observation'
                out.write(f"- {item['release_tag']}: {before} → {item['after']['state']}; build {item['after']['build_id']}; processing {item['after']['processing']}\n")
        else:
            out.write(f'No App Store state changes. Refreshed {len(unchanged)} releases.\n')
        if skipped:
            out.write(f'\nRetained {len(skipped)} older completed snapshots. Manual status refresh checks them again.\n')


if __name__ == '__main__':
    main()
