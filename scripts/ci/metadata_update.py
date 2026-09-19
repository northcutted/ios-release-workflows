"""Resolve metadata from one reviewed main commit, without checking out its code."""
import base64
import os
from pathlib import Path
import re
import shutil
from configuration import CONFIG, require
from evidence import digest, metadata, write
from fetch import api

sha = os.environ['METADATA_COMMIT']
require(re.fullmatch(r'[a-f0-9]{40}', sha), 'Metadata-only updates require an exact commit')
prefix = f"repos/{CONFIG['repository']}"
require(api(f'{prefix}/compare/{sha}...main')['status'] in ('ahead', 'identical'), 'Metadata commit is not on protected main')
root = Path('release-assets/metadata')
shutil.rmtree(root)
root.mkdir(parents=True)
records = []
for locale in CONFIG['locales']:
    entries = api(f"{prefix}/contents/{CONFIG['metadata_path']}/{locale}?ref={sha}")
    for entry in entries:
        require(entry['type'] == 'file' and re.fullmatch(r'[A-Za-z0-9_]+\.txt', entry['name']), 'Unexpected metadata entry')
        data = api(f"{prefix}/contents/{CONFIG['metadata_path']}/{locale}/{entry['name']}?ref={sha}")
        require(data['encoding'] == 'base64', 'Unsupported metadata encoding')
        destination = root / locale / entry['name']
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(base64.b64decode(data['content']))
        records.append({'path': f"{locale}/{entry['name']}", 'sha256': digest(destination)})
metadata(root)
write('build/metadata-update.json', {'source_sha': sha, 'files': records})
