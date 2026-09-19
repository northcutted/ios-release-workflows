import hashlib
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import install_macos_verifier as installer


class VerifierBootstrapTests(unittest.TestCase):
    def test_tampered_binary_or_provenance_never_executes_or_enters_path(self):
        pins = {'arm64': tuple(hashlib.sha256(value).hexdigest() for value in (b'binary', b'provenance'))}
        for tampered in ('binary', 'provenance'):
            with self.subTest(tampered=tampered), tempfile.TemporaryDirectory() as directory:
                path_file = Path(directory)/'github-path'
                def command(args, **kwargs):
                    self.assertEqual(args[0], 'curl', 'Unverified binary was executed')
                    target = Path(args[args.index('--output')+1])
                    kind = 'binary' if target.name == 'slsa-verifier' else 'provenance'
                    target.write_bytes(b'tampered' if kind == tampered else kind.encode())
                with patch.dict(os.environ, {'RUNNER_TEMP': directory, 'GITHUB_PATH': str(path_file)}), \
                     patch.object(installer, 'DIGESTS', pins), \
                     patch('install_macos_verifier.platform.system', return_value='Darwin'), \
                     patch('install_macos_verifier.platform.machine', return_value='arm64'), \
                     patch('install_macos_verifier.subprocess.run', side_effect=command):
                    with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                        installer.install()
                self.assertFalse(path_file.exists())

    def test_unsupported_platform_does_not_download_or_execute(self):
        with patch('install_macos_verifier.platform.system', return_value='Linux'), \
             patch('install_macos_verifier.subprocess.run') as run:
            with self.assertRaises(ValueError): installer.install()
            run.assert_not_called()
