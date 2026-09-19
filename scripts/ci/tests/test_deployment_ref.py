import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
os.environ.setdefault('IOS_RELEASE_CONFIG', str(Path(__file__).resolve().parents[3] / 'examples/picstrip.json'))
from deployment_ref import create, verify_ref

class DeploymentRefTests(unittest.TestCase):
    def test_only_exact_release_or_reviewed_protected_recovery_tag_is_eligible(self):
        sha='a'*40; ref=f'refs/tags/v1.7.0-deploy-{sha}'
        read=lambda _: {'status': 'ahead'}
        self.assertEqual('v1.7.0', verify_ref('v1.7.0', 'refs/tags/v1.7.0', sha, 'release', read))
        self.assertEqual('v1.7.0', verify_ref('v1.7.0', ref, sha, 'create', read))
        for tag, candidate, source, event in [('v1.8.0', ref, sha, 'create'), ('v1.7.0', 'refs/heads/main', sha, 'workflow_dispatch'), ('v1.7.0', ref, 'b'*40, 'create'), ('v1.7.0', ref, sha, 'pull_request')]:
            with self.assertRaises(ValueError): verify_ref(tag, candidate, source, event, read)
        for status in ('behind', 'diverged'):
            with self.assertRaises(ValueError): verify_ref('v1.7.0', ref, sha, 'create', lambda _: {'status': status})

    def test_recovery_tag_requires_immutable_publication_and_never_moves_existing_tag(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory)/'release-manifest.json').write_text(json.dumps({'tag': 'v1.7.0', 'source_sha': 'b'*40}))
            for immutable in (False, True):
                read=lambda path: {'status': 'ahead'} if '/compare/' in path else {'immutable': immutable, 'draft': False}
                with patch('deployment_ref.subprocess.run', return_value=subprocess.CompletedProcess([], 0, '{}', '')), patch('deployment_ref.tag_commit', return_value='a'*40), patch('deployment_ref.gh') as mutation:
                    if immutable: create(directory, 'a'*40, read)
                    else:
                        with self.assertRaises(ValueError): create(directory, 'a'*40, read)
                    mutation.assert_not_called()
            with patch('deployment_ref.subprocess.run', return_value=subprocess.CompletedProcess([], 0, '{}', '')), patch('deployment_ref.tag_commit', return_value='c'*40), patch('deployment_ref.gh') as mutation:
                with self.assertRaises(ValueError): create(directory, 'a'*40, read)
                mutation.assert_not_called()
