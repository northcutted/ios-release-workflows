import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from ios_release.tools import actionlint


class ToolsTests(unittest.TestCase):
    def archive(self, symlink=False):
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w:gz") as archive:
            member = tarfile.TarInfo("actionlint")
            if symlink:
                member.type, member.linkname = tarfile.SYMTYPE, "/outside"
                archive.addfile(member)
            else:
                member.size = 4
                archive.addfile(member, io.BytesIO(b"tool"))
        return stream.getvalue()

    def test_distribution_hash_and_cached_executable_are_checked(self):
        data = self.archive()
        digest = hashlib.sha256(data).hexdigest()
        with tempfile.TemporaryDirectory() as directory, patch.dict("os.environ", {"RUNNER_TEMP": directory}), patch("ios_release.tools.platform.system", return_value="Darwin"), patch("ios_release.tools.platform.machine", return_value="arm64"), patch("ios_release.tools.ACTIONLINT_HASHES", {("Darwin", "arm64"): digest}), patch("ios_release.tools.urlopen", return_value=io.BytesIO(data)) as fetch:
            path = actionlint(directory)
            self.assertEqual(path.read_bytes(), b"tool")
            path.write_bytes(b"corrupt")
            self.assertEqual(actionlint(directory).read_bytes(), b"tool")
            fetch.assert_called_once()

    def test_wrong_hash_and_symlinks_are_rejected(self):
        for data, digest, message in [(b"wrong", "a" * 64, "checksum"), (self.archive(True), hashlib.sha256(self.archive(True)).hexdigest(), "regular file")]:
            with tempfile.TemporaryDirectory() as directory, patch.dict("os.environ", {"RUNNER_TEMP": directory}), patch("ios_release.tools.platform.system", return_value="Darwin"), patch("ios_release.tools.platform.machine", return_value="arm64"), patch("ios_release.tools.ACTIONLINT_HASHES", {("Darwin", "arm64"): digest}), patch("ios_release.tools.urlopen", return_value=io.BytesIO(data)):
                with self.assertRaisesRegex(ValueError, message):
                    actionlint(directory)
