import json
import os
from pathlib import Path
from configuration import CONFIG, require

version = json.loads(Path('build/version/semantic-release.json').read_text())
require(version['source_sha'] == os.environ['GITHUB_SHA'], 'Wrong version source')
number = int(os.environ['GITHUB_RUN_NUMBER']) + CONFIG['build_number_offset']
attempt = int(os.environ['GITHUB_RUN_ATTEMPT'])
require(0 < number <= 9999 and 0 < attempt <= 99, 'Apple build number range exhausted')
values = dict(version=version['version'], source_sha=version['source_sha'], build_number=f'{number}.{attempt}',
              run_id=os.environ['GITHUB_RUN_ID'], run_attempt=attempt, prefix=f"{os.environ['GITHUB_RUN_ID']}-{attempt}",
              candidate=str(version['will_release'] or os.environ['GITHUB_EVENT_NAME'] == 'workflow_dispatch').lower())
with open(os.environ['GITHUB_OUTPUT'], 'a') as out:
    for key, value in values.items(): out.write(f'{key}={value}\n')
with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as out:
    out.write(f"Preparing {version['version']} ({number}.{attempt}); no distribution occurs in this workflow.\n")
