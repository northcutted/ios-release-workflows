import copy
import json
import os
from pathlib import Path
import sys
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
        command=native_command('/tmp/evidence')
        self.assertIn('github.com/northcutted/ios-release-workflows/.github/workflows/prepare.yml',command)
        self.assertEqual(command[command.index('--signer-digest')+1],'f'*40)
        self.assertNotIn('--source-digest',command)
    def test_missing_localization_check_is_not_green_evidence(self):
        checks={name:{'status':0,'executed':1,'failures':0} for name in evidence.CHECKS if name!='localization'}
        with self.assertRaises(ValueError):evidence.validate_qa_summary({'source_sha':'a'*40,'checks':checks},'a'*40)
    def test_shipped_dependency_policy_is_explicit(self):
        raw=json.loads((ROOT/'examples/minimal.json').read_text());raw.pop('runtime_dependencies')
        with tempfile.TemporaryDirectory() as directory,patch.dict(os.environ,{'GITHUB_REPOSITORY':raw['repository']}):
            path=Path(directory)/'config.json';path.write_text(json.dumps(raw))
            with self.assertRaises(ValueError):load(path)
