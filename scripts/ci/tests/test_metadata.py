import base64
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
os.environ.setdefault('IOS_RELEASE_CONFIG',str(Path(__file__).resolve().parents[3]/'examples/picstrip.json'))
import metadata_update as subject

class MetadataTests(unittest.TestCase):
    def test_exact_commit_supplies_global_and_localized_metadata_with_hashes(self):
        sha='a'*40;requests=[]
        def read(path):
            requests.append(path)
            if '/compare/' in path:return {'status':'ahead'}
            self.assertTrue(path.endswith('?ref='+sha))
            if path.endswith('/metadata?ref='+sha):return [{'type':'file','name':'copyright.txt'},{'type':'dir','name':'en-US'}]
            if path.endswith('/en-US?ref='+sha):return [{'type':'file','name':'description.txt'}]
            return {'encoding':'base64','content':base64.b64encode(b'reviewed text').decode()}
        previous=Path.cwd()
        with tempfile.TemporaryDirectory() as directory:
            try:
                os.chdir(directory);Path('release-assets/metadata').mkdir(parents=True)
                with patch.dict(subject.CONFIG,{'locales':['en-US']}),patch.object(subject,'api',side_effect=read),patch.object(subject,'metadata'):
                    subject.update(sha)
                record=json.loads(Path('build/metadata-update.json').read_text())
                self.assertEqual({f['path'] for f in record['files']},{'copyright.txt','en-US/description.txt'})
                self.assertEqual(Path('release-assets/metadata/copyright.txt').read_text(),'reviewed text')
                with patch.object(subject,'api',return_value={'status':'diverged'}):
                    with self.assertRaises(ValueError):subject.update(sha)
            finally:os.chdir(previous)
