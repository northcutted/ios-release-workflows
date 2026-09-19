#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
from evidence import CONFIG, asset, metadata, screenshots, write, require


def compressed(directory, prefix, target):
    with tarfile.open(str(target) + '.tar', 'w') as tar:
        for path in sorted(Path(directory).rglob('*')):
            require(not path.is_symlink(), 'Release input links are forbidden')
            if path.is_file():
                tar.add(path, arcname=prefix + '/' + path.relative_to(directory).as_posix(), recursive=False)
    subprocess.run(['zstd', '-6', '-T2', '--rm', str(target) + '.tar', '-o', str(target)], check=True)


def package():
    root = Path('build/package'); root.mkdir(parents=True, exist_ok=True)
    version = json.loads(Path('build/version/semantic-release.json').read_text())
    require(version['source_sha'] == os.environ['SOURCE_SHA'], 'Source/version mismatch')
    (root / 'release-notes.md').write_text(version['notes'] + '\n')
    shutil.copyfile(os.environ['IOS_RELEASE_CONFIG'], root / 'app-config.json')
    metadata(CONFIG['metadata_path'])
    screenshots(CONFIG['screenshots_path'], root / 'screenshots-manifest.json')
    compressed(CONFIG['metadata_path'], 'metadata', root / 'app-store-metadata.tar.zst')
    compressed(CONFIG['screenshots_path'], 'screenshots', root / 'app-store-screenshots.tar.zst')
    if CONFIG.get('accessibility_path'):
        shutil.copyfile(CONFIG['accessibility_path'], root / 'accessibility.json')
    with (root / 'release-source.tar.zst').open('wb') as output:
        archive = subprocess.Popen(['git', 'archive', '--format=tar', '--prefix=source/', os.environ['SOURCE_SHA']], stdout=subprocess.PIPE)
        subprocess.run(['zstd', '-6', '-T2'], stdin=archive.stdout, stdout=output, check=True)
        require(archive.wait() == 0, 'Source packaging failed')
    write(root / 'package-checksums.json', [asset(p) for p in sorted(root.iterdir()) if p.is_file() and p.name != 'package-checksums.json'])


if __name__ == '__main__':
    package()
