import copy
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
os.environ.setdefault('IOS_RELEASE_CONFIG',str(Path(__file__).resolve().parents[3]/'examples/picstrip.json'))
os.environ.setdefault('IOS_RELEASE_REVISION','f'*40)
from configuration import load
from verify_release import native_command
import evidence

ROOT=Path(__file__).resolve().parents[3]
class ConsumerContractTests(unittest.TestCase):
    def test_unrelated_repositories_need_no_picstrip_files(self):
        for name,count in [('minimal',1),('extensions',3)]:
            raw=json.loads((ROOT/f'examples/{name}.json').read_text())
            with patch.dict(os.environ,{'GITHUB_REPOSITORY':raw['repository']}):
                config=load(ROOT/f'examples/{name}.json')
                self.assertEqual(len(config['targets']),count)
                self.assertNotIn('PicStrip',json.dumps(config))
                self.assertEqual(config['targets'][0]['entitlements'],{})
            with patch.dict(os.environ,{'GITHUB_REPOSITORY':'attacker/other'}):
                with self.assertRaises(ValueError):load(ROOT/f'examples/{name}.json')
    def test_consumer_and_producer_commits_are_independent(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory)/'release-build-manifest.json').write_text(json.dumps({'producer': {'revision': 'f'*40}}))
            command=native_command(directory)
        self.assertIn('github.com/northcutted/ios-release-workflows/.github/workflows/prepare.yml',command)
        self.assertEqual(command[command.index('--signer-digest')+1],'f'*40)
        self.assertNotIn('--source-digest',command)
    def test_only_explicitly_approved_historical_signers_are_accepted(self):
        old='b'*40
        with patch.dict(os.environ, {'IOS_RELEASE_TRUSTED_PRODUCER_REVISIONS': json.dumps(['f'*40])}):
            with self.assertRaisesRegex(ValueError, 'unapproved'):
                evidence.trusted_revision(old)
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
                'IOS_RELEASE_TRUSTED_PRODUCER_REVISIONS': json.dumps(['f'*40, old])}):
            root=Path(directory)
            (root/'release-build-manifest.json').write_text(json.dumps({'producer': {'revision': old}}))
            (root/'release-manifest.json').write_text(json.dumps({'producer': {'revision': old}, 'promotion': {'revision': 'f'*40}}))
            for final, expected in ((False, old), (True, 'f'*40)):
                command=native_command(root, final)
                self.assertEqual(command[command.index('--signer-digest')+1], expected)
            (root/'release-build-manifest.json').write_text(json.dumps({'producer': {'revision': 'c'*40}, 'trusted_producer_revisions': ['c'*40]}))
            with self.assertRaisesRegex(ValueError, 'unapproved'):
                native_command(root)
        for policy in ([], ['main'], ['b'*39], 'b'*40):
            with patch.dict(os.environ, {'IOS_RELEASE_TRUSTED_PRODUCER_REVISIONS': json.dumps(policy)}):
                with self.assertRaises(ValueError): evidence.trusted_revision(old)

    def test_bootstrap_freezes_producer_policy_before_artifact_config_is_loaded(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); config=root/'app.json'; output=root/'environment'
            raw=json.loads((ROOT/'examples/minimal.json').read_text())
            raw['trusted_producer_revisions']=['b'*40]
            config.write_text(json.dumps(raw))
            env={**os.environ, 'GITHUB_WORKSPACE': directory, 'CONFIG_PATH': 'app.json',
                 'PLATFORM_REVISION': 'f'*40, 'RUNNER_TEMP': directory, 'GITHUB_ENV': str(output),
                 'GITHUB_REPOSITORY': raw['repository']}
            subprocess.run([sys.executable, str(ROOT/'actions/bootstrap/bootstrap.py')], env=env, check=True)
            values=dict(line.split('=', 1) for line in output.read_text().splitlines())
            self.assertEqual(json.loads(values['IOS_RELEASE_TRUSTED_PRODUCER_REVISIONS']), ['b'*40, 'f'*40])
            raw['trusted_producer_revisions']=['c'*40]; config.write_text(json.dumps(raw))
            with patch.dict(os.environ, values):
                self.assertEqual(evidence.trusted_revision('b'*40), 'b'*40)
                with self.assertRaises(ValueError): evidence.trusted_revision('c'*40)
            raw['trusted_producer_revisions']=['main']; config.write_text(json.dumps(raw))
            result=subprocess.run([sys.executable, str(ROOT/'actions/bootstrap/bootstrap.py')], env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
    def test_missing_localization_check_is_not_green_evidence(self):
        checks={name:{'status':0,'executed':1,'failures':0} for name in evidence.CHECKS if name!='localization'}
        with self.assertRaises(ValueError):evidence.validate_qa_summary({'source_sha':'a'*40,'checks':checks},'a'*40)
    def test_shipped_dependency_policy_is_explicit(self):
        raw=json.loads((ROOT/'examples/minimal.json').read_text());raw.pop('runtime_dependencies')
        with tempfile.TemporaryDirectory() as directory,patch.dict(os.environ,{'GITHUB_REPOSITORY':raw['repository']}):
            path=Path(directory)/'config.json';path.write_text(json.dumps(raw))
            with self.assertRaises(ValueError):load(path)
