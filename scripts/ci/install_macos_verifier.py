"""Bootstrap macOS from reviewed upstream release digests, then verify provenance."""
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import tempfile

VERSION = 'v2.7.1'
# Official release assets, independently provenance-verified before pinning.
DIGESTS = {
    'arm64': ('39abfcf5f1d690c3e889ce3d2d6a8b87711424d83368511868d414e8f8bcb05c',
              'b5310f8519880b165836c644f38c7bd6e583ce0f26dd3a15548cd4bf7e69e435'),
    'x86_64': ('4baf25415727821f847a38bccedc86c3e5b17cbfc2eb534cd554feb6c856d6f1',
               '2826ce37d337978da952179cd9450466458fcd71766060c80bb2851ea345ad2f'),
}


def install():
    machine = platform.machine()
    if platform.system() != 'Darwin' or machine not in DIGESTS:
        raise ValueError('Unsupported macOS verifier architecture')
    arch = 'arm64' if machine == 'arm64' else 'amd64'
    directory = Path(tempfile.mkdtemp(prefix='verified-slsa-', dir=os.environ['RUNNER_TEMP']))
    binary = directory / 'slsa-verifier'
    provenance = directory / 'provenance.intoto.jsonl'
    asset = 'slsa-verifier-darwin-' + arch
    for name, destination, expected in zip((asset, asset + '.intoto.jsonl'),
                                            (binary, provenance), DIGESTS[machine]):
        subprocess.run(['curl', '--fail', '--location', '--silent', '--show-error',
                        '--proto', '=https', '--tlsv1.2', '--retry', '3', '--max-time', '180',
                        f'https://github.com/slsa-framework/slsa-verifier/releases/download/{VERSION}/{name}',
                        '--output', str(destination)], check=True)
        with destination.open('rb') as file:
            actual = hashlib.file_digest(file, 'sha256').hexdigest()
        if actual != expected:
            raise ValueError('SLSA verifier bootstrap checksum mismatch: ' + name)
    binary.chmod(0o700)
    subprocess.run([str(binary), 'verify-artifact', str(binary), '--provenance-path', str(provenance),
                    '--source-uri', 'github.com/slsa-framework/slsa-verifier', '--source-tag', VERSION], check=True)
    with open(os.environ['GITHUB_PATH'], 'a') as output:
        output.write(str(directory) + '\n')


if __name__ == '__main__':
    install()
