import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts/ci'))
os.environ.setdefault('IOS_RELEASE_CONFIG', str(ROOT / 'examples/picstrip.json'))
from configuration import load


class ReplacementTests(unittest.TestCase):
    def setUp(self):
        self.config = json.loads((ROOT / 'examples/picstrip.json').read_text())
        self.config['replacement_release'] = {'version': '1.7.0', 'source_tag': 'v1.7.0', 'build_number': '77.1', 'app_store_build_id': 'previous'}

    def test_configuration_requires_exact_old_identity(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {'GITHUB_REPOSITORY': self.config['repository']}):
            path = Path(directory) / 'config.json'
            path.write_text(json.dumps(self.config)); load(path)
            for change in ({'source_tag': 'v1.6.5'}, {'version': '1.7.0-beta'}, {'build_number': '0.1'}, {'app_store_build_id': '../foreign'}, {'unexpected': True}):
                config = copy.deepcopy(self.config)
                config['replacement_release'].update(change)
                path.write_text(json.dumps(config))
                with self.assertRaises(ValueError): load(path)

    def context(self, old_build='77.1', version='1.7.0', previous='v1.7.0'):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = copy.deepcopy(self.config); config['replacement_release']['build_number'] = old_build
            (root / 'config.json').write_text(json.dumps(config))
            (root / 'build/version').mkdir(parents=True)
            path = root / 'build/version/semantic-release.json'
            path.write_text(json.dumps({'source_sha': 'a' * 40, 'version': version, 'git_tag': None, 'replacement_for': previous, 'will_release': True}))
            env = {**os.environ, 'IOS_RELEASE_CONFIG': str(root / 'config.json'), 'GITHUB_REPOSITORY': config['repository'],
                   'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_NUMBER': '100', 'GITHUB_RUN_ATTEMPT': '1', 'GITHUB_RUN_ID': '900',
                   'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_OUTPUT': str(root / 'output'), 'GITHUB_STEP_SUMMARY': str(root / 'summary')}
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/ci/candidate_context.py')], cwd=root, env=env, capture_output=True)
            return result.returncode, json.loads(path.read_text()), (root / 'output').read_text() if (root / 'output').exists() else ''

    def test_context_generates_exact_immutable_build_tag(self):
        status, version, outputs = self.context()
        self.assertEqual(0, status)
        self.assertEqual('v1.7.0-build-100.1', version['git_tag'])
        self.assertIn('version=1.7.0\n', outputs)
        self.assertIn('build_number=100.1\n', outputs)

    def test_context_rejects_non_newer_or_conflicting_replacements(self):
        for arguments in ({'old_build': '100.1'}, {'old_build': '101.1'}, {'version': '1.8.0'}, {'previous': None}):
            status, _, outputs = self.context(**arguments)
            self.assertNotEqual(0, status)
            self.assertEqual('', outputs)
