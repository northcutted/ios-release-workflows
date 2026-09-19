import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
os.environ.setdefault("IOS_RELEASE_CONFIG", str(Path(__file__).resolve().parents[3] / "examples/picstrip.json"))
from validate_ipa import signing_certificate_digest


@unittest.skipUnless(sys.platform == "darwin", "Requires the real macOS codesign tool")
class CodesignTests(unittest.TestCase):
    def test_extracts_a_real_signed_application_certificate(self):
        developer = Path(subprocess.check_output(["xcode-select", "-p"], text=True).strip())
        application = developer.parent.parent
        self.assertEqual(application.suffix, ".app", "The macOS test runner requires full Xcode")
        with tempfile.TemporaryDirectory(prefix="certificate extraction ") as directory:
            prefix = Path(directory) / "signer-"
            actual = signing_certificate_digest(application, prefix)
            certificate = Path(str(prefix) + "0")
            subprocess.run(["openssl", "x509", "-inform", "DER", "-in", str(certificate), "-noout"], check=True)
            self.assertEqual(actual, hashlib.sha256(certificate.read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
