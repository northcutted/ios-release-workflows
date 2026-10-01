import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from ios_release.version import analyze, filter_reverts, notes, parse_commit, policy, release_type, bump

ROOT = Path(__file__).resolve().parents[1]


class VersionTests(unittest.TestCase):
    def test_locked_node_reference_decisions_and_notes(self):
        settings = policy(ROOT)
        fixtures = json.loads((ROOT / "tests/fixtures/version-reference.json").read_text())
        for fixture in fixtures["cases"]:
            with self.subTest(case=fixture["name"]):
                commits = filter_reverts([parse_commit(commit) for commit in fixture["commits"]])
                release = release_type(commits, settings["release_rules"])
                expected = fixture["expected"]
                self.assertEqual(bool(release) or fixture["name"] == "replacement", expected["will_release"])
                candidate = "1.6.5" if fixture["name"] == "replacement" or not release else bump("1.6.5", release)
                self.assertEqual(candidate, expected["version"])
                rendered = notes(commits, settings["note_types"], expected["version"], expected["git_tag"], expected["source_sha"], "v1.6.5", "example/fixture") if release else "Verification build; no release changes."
                self.assertEqual(re.sub(r"\(\d{4}-\d{2}-\d{2}\)", "(DATE)", rendered), expected["notes"])

    def fixture(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        def git(*args):
            return subprocess.check_output(["git", *args], cwd=root, text=True, stderr=subprocess.DEVNULL).strip()
        git("init", "-b", "main")
        git("config", "user.name", "CI Test")
        git("config", "user.email", "ci@example.invalid")
        shutil.copy(ROOT / "tests/fixtures/legacy-release-policy.json", root / ".releaserc.json")
        git("add", ".")
        git("commit", "-m", "chore: initial")
        git("tag", "v1.6.5")
        return root, git, lambda message: git("commit", "--allow-empty", "-m", message)

    def test_read_only_versions_and_reachable_stable_tags(self):
        root, git, commit = self.fixture()
        with patch.dict("os.environ", {"IOS_RELEASE_CONFIG": "missing.json"}):
            commit("docs: explain usage")
            self.assertFalse(analyze(root)["will_release"])
            commit("fix: reject bytes")
            self.assertEqual(analyze(root)["version"], "1.6.6")
            commit("feat: add support")
            self.assertEqual(analyze(root)["version"], "1.7.0")
            commit("feat!: change contract")
            self.assertEqual(analyze(root)["version"], "2.0.0")
            git("checkout", "-b", "unrelated")
            commit("feat: other branch")
            git("tag", "v99.0.0")
            git("checkout", "main")
            git("tag", "v9.0.0-beta.1")
            self.assertEqual(analyze(root)["version"], "2.0.0")
            self.assertEqual(git("status", "--porcelain"), "")
            self.assertEqual(git("rev-parse", "HEAD"), analyze(root)["source_sha"])

    def test_replacements_preserve_tag_and_reject_older_or_missing_versions(self):
        root, git, commit = self.fixture()
        commit("feat: privacy review")
        before = git("rev-parse", "v1.6.5")
        for version, tag in [("1.5.0", "v1.5.0"), ("1.7.0", "v1.7.0"), ("1.6.5", "v1.5.0"), ("1.6.5-beta.1", "v1.6.5-beta.1")]:
            (root / "replacement.json").write_text(json.dumps({"replacement_release": {"version": version, "source_tag": tag}}))
            with self.assertRaisesRegex(ValueError, "highest reachable stable release"):
                analyze(root, "replacement.json")
        (root / "replacement.json").write_text(json.dumps({"replacement_release": {"version": "1.6.5", "source_tag": "v1.6.5"}}))
        result = analyze(root, "replacement.json")
        self.assertEqual(result["version"], "1.6.5")
        self.assertIsNone(result["git_tag"])
        self.assertTrue(result["will_release"])
        self.assertEqual(git("rev-parse", "v1.6.5"), before)

    def test_new_policy_and_unsupported_options(self):
        root, _, _ = self.fixture()
        settings = policy(root)
        (root / ".github").mkdir()
        target = root / ".github/ios-version.json"
        target.write_text(json.dumps(settings))
        self.assertEqual(policy(root), settings)
        settings["release_rules"].append({"plugin": "custom", "release": "patch"})
        target.write_text(json.dumps(settings))
        with self.assertRaisesRegex(ValueError, "Unsupported release rule"):
            policy(root)
