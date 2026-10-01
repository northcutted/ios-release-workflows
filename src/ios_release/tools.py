"""Install checksum-pinned standalone tools in the task's own directory."""
import hashlib
import io
import os
from pathlib import Path
import platform
import tarfile
import tempfile
from urllib.request import urlopen

ACTIONLINT_VERSION = "1.7.12"
ACTIONLINT_HASHES = {
    ("Darwin", "arm64"): "aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f",
    ("Darwin", "x86_64"): "5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644",
    ("Linux", "x86_64"): "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8",
    ("Linux", "aarch64"): "325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6",
}


def actionlint(app):
    system, architecture = platform.system(), platform.machine()
    digest = ACTIONLINT_HASHES.get((system, architecture))
    if not digest:
        raise ValueError(f"Unsupported actionlint host: {system}/{architecture}")
    directory = Path(os.environ.get("RUNNER_TEMP", Path(app) / "build")) / "ios-release-tools" / digest
    directory.mkdir(parents=True, exist_ok=True)
    archive_path, executable = directory / "archive.tar.gz", directory / "actionlint"
    data = archive_path.read_bytes() if archive_path.exists() else b""
    if hashlib.sha256(data).hexdigest() != digest:
        arch = "arm64" if architecture in {"arm64", "aarch64"} else "amd64"
        url = f"https://github.com/rhysd/actionlint/releases/download/v{ACTIONLINT_VERSION}/actionlint_{ACTIONLINT_VERSION}_{system.lower()}_{arch}.tar.gz"
        with urlopen(url, timeout=30) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError("actionlint distribution checksum mismatch")
        archive_path.write_bytes(data)
    # Read one regular member; never extract archive paths or symlinks.
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        member = archive.getmember("actionlint")
        if not member.isfile():
            raise ValueError("actionlint executable must be a regular file")
        binary = archive.extractfile(member).read()
    if not executable.exists() or executable.read_bytes() != binary:
        with tempfile.NamedTemporaryFile(dir=directory, delete=False) as temp:
            temp.write(binary)
            name = temp.name
        os.chmod(name, 0o755)
        os.replace(name, executable)
    return executable
