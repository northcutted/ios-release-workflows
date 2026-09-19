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
import fetch

class ProcessedHandoffTests(unittest.TestCase):
    def test_matching_signed_canary_is_reused_but_substitution_stops(self):
        candidate={'source_sha':'a'*40,'candidate_id':'candidate'}
        final={**candidate,'promotion':{'run_id':'44','run_attempt':'2'}}
        artifact={'expired':False,'digest':'sha256:'+'b'*64,'workflow_run':{'id':44},'name':'final-44-2'}
        def extract(_id,_digest,root):
            (Path(root)/'testflight-status.json').write_text('{"app_store_build_id":"recorded"}')
        previous=Path.cwd()
        with tempfile.TemporaryDirectory() as directory:
            try:
                os.chdir(directory);Path('release-assets').mkdir()
                for mode in ('valid','candidate','run','attempt','digest','signature'):
                    current=json.loads(json.dumps(final));entry=json.loads(json.dumps(artifact))
                    if mode=='candidate':current['candidate_id']='other'
                    if mode=='run':entry['workflow_run']['id']=45
                    if mode=='attempt':entry['name']='final-44-1'
                    if mode=='digest':entry['digest']='sha256:'+'c'*64
                    side=[candidate,RuntimeError('bad signature') if mode=='signature' else current]
                    with self.subTest(mode=mode),patch.object(fetch,'verify',side_effect=side),patch.object(fetch,'api',return_value=entry),patch.object(fetch,'extract_artifact',side_effect=extract):
                        if mode=='valid':
                            fetch.processed('1','b'*64)
                            self.assertEqual(json.loads(Path('release-assets/testflight-status.json').read_text())['app_store_build_id'],'recorded')
                        else:
                            with self.assertRaises((ValueError,RuntimeError)):fetch.processed('1','b'*64)
            finally:os.chdir(previous)
