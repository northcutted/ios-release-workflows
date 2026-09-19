"""Restore the latest immutable operation artifact from this same workflow run.

Rerunning only failed jobs increments run_attempt without rerunning producers.
Search by exact run and producer prefix, never by an unrelated run or 'latest'.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import zipfile
from evidence import digest, require

prefix, destination = sys.argv[1:3]
optional = "--optional" in sys.argv[3:]
require(re.fullmatch(r'[a-z-]+', prefix), 'Invalid operation prefix')
run_id = os.environ['GITHUB_RUN_ID']; attempt = int(os.environ['GITHUB_RUN_ATTEMPT'])
pattern = re.compile(re.escape(prefix + '-' + run_id) + r'-(\d+)$')
pages = json.loads(subprocess.check_output(['gh', 'api', '--paginate', '--slurp', f"repos/{os.environ['GITHUB_REPOSITORY']}/actions/runs/{run_id}/artifacts?per_page=100"], text=True))
artifacts = [(int(match[1]), a) for page in pages for a in page['artifacts'] if (match := pattern.fullmatch(a['name'])) and int(match[1]) <= attempt and not a['expired']]
if not artifacts and optional:
    print('No earlier operation receipt in this run')
    sys.exit(0)
require(artifacts, 'No successful producer artifact in this run; rerun the producing job')
_, artifact = max(artifacts, key=lambda pair: pair[0])
Path(destination).mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory() as temp:
    archive = Path(temp) / 'receipt.zip'
    with archive.open('wb') as out:
        subprocess.run(['gh', 'api', f"repos/{os.environ['GITHUB_REPOSITORY']}/actions/artifacts/{artifact['id']}/zip"], stdout=out, check=True)
    require(artifact['digest'] == 'sha256:' + digest(archive), 'Operation artifact digest mismatch')
    with zipfile.ZipFile(archive) as zipped:
        require(sum(i.file_size for i in zipped.infolist()) < 4 * 1024**3, 'Operation archive too large')
        names=set()
        for item in zipped.infolist():
            require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', item.filename) and item.filename not in names, 'Unsafe/duplicate receipt member')
            require((item.external_attr >> 16) & 0o170000 != 0o120000, 'Receipt symlink')
            names.add(item.filename)
        zipped.extractall(destination)
