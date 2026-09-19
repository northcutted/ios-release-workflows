#!/usr/bin/env python3
from pathlib import Path
import subprocess
import tempfile
from evidence import extract

for name, prefix in [('app-store-metadata', 'metadata'), ('app-store-screenshots', 'screenshots')]:
    with tempfile.TemporaryDirectory() as directory:
        tar = Path(directory) / 'assets.tar'
        with tar.open('wb') as output:
            subprocess.run(['zstd', '-d', '-c', f'release-assets/{name}.tar.zst'], stdout=output, check=True)
        extract(tar, 'release-assets', prefix)
