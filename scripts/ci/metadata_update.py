"""Resolve public metadata from one reviewed main commit, without executing its code."""
import base64
import os
from pathlib import Path
import re
import shutil
from configuration import CONFIG, require
from evidence import digest, metadata, write
from fetch import api

ROOT_FIELDS = {'copyright.txt', 'primary_category.txt', 'secondary_category.txt',
               'primary_first_sub_category.txt', 'primary_second_sub_category.txt',
               'secondary_first_sub_category.txt', 'secondary_second_sub_category.txt'}


def update(sha):
    require(re.fullmatch(r'[a-f0-9]{40}', sha), 'Metadata-only updates require an exact commit')
    prefix = f"repos/{CONFIG['repository']}"
    require(api(f'{prefix}/compare/{sha}...main')['status'] in ('ahead', 'identical'), 'Metadata commit is not on protected main')
    root = Path('release-assets/metadata')
    shutil.rmtree(root)
    root.mkdir(parents=True)
    records = []
    for directory in [''] + CONFIG['locales']:
        base = f"{prefix}/contents/{CONFIG['metadata_path']}" + (f'/{directory}' if directory else '')
        entries = api(f"{base}?ref={sha}")
        for entry in entries:
            if not directory and entry['name'] not in ROOT_FIELDS:
                continue
            require(entry['type'] == 'file' and re.fullmatch(r'[A-Za-z0-9_]+\.txt', entry['name']), 'Unexpected metadata entry')
            data = api(f"{base}/{entry['name']}?ref={sha}")
            require(data['encoding'] == 'base64', 'Unsupported metadata encoding')
            relative = Path(directory) / entry['name']
            destination = root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(base64.b64decode(data['content'], validate=False))
            records.append({'path': relative.as_posix(), 'sha256': digest(destination)})
    metadata(root)
    write('build/metadata-update.json', {'source_sha': sha, 'files': records})


if __name__ == '__main__':
    update(os.environ['METADATA_COMMIT'])
