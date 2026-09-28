import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
os.environ.setdefault('IOS_RELEASE_CONFIG', str(Path(__file__).resolve().parents[3] / 'examples/picstrip.json'))
os.environ.setdefault('IOS_RELEASE_REVISION', 'f' * 40)
import resolve_release as release


class ReleaseSelectionTests(unittest.TestCase):
    def setUp(self):
        self.repo = release.CONFIG['repository']
        self.run = {'id': 123, 'run_number': 86, 'status': 'completed', 'conclusion': 'success',
                    'head_branch': 'main', 'path': release.CONFIG['source_workflow'], 'event': 'push',
                    'head_sha': 'a' * 40, 'repository': {'full_name': self.repo}, 'head_repository': {'full_name': self.repo}}
        self.artifact = {'id': 456, 'name': 'candidate-123-1', 'expired': False, 'digest': 'sha256:' + 'b' * 64,
                         'workflow_run': {'id': 123}}
        self.manifest = {'source_sha': 'a' * 40, 'version': '1.7.0', 'build_number': '86.1',
                         'ipa_sha256': 'c' * 64, 'candidate_id': 'exact-candidate', 'tag': 'v1.7.0-build-86.1',
                         'run_id': '123', 'run_attempt': '1'}

    def read(self, path):
        if '/compare/' in path:
            return {'status': 'ahead'}
        if '/artifacts?' in path:
            return {'artifacts': [self.artifact]}
        if '/workflows/main.yml/runs?' in path:
            return {'workflow_runs': [self.run]}
        return self.run

    def test_only_exact_same_repository_run_selectors_are_accepted(self):
        for source in ('123', f'https://github.com/{self.repo}/actions/runs/123', '#86'):
            self.assertEqual(release.run_id(source, self.read), '123')
        for source in ('latest', '0', '../123', f'https://evil.invalid/{self.repo}/actions/runs/123',
                       'https://github.com/other/app/actions/runs/123', '123\nsha256=bad', '#0'):
            with self.subTest(source=source), self.assertRaises(ValueError):
                release.run_id(source, self.read)

    def test_failed_incomplete_forked_and_wrong_workflow_runs_stop_selection(self):
        variants = [('status', 'in_progress'), ('conclusion', 'failure'), ('head_branch', 'feature'),
                    ('path', '.github/workflows/pr.yml'), ('event', 'pull_request'), ('head_sha', 'invalid'),
                    ('repository', {'full_name': 'other/app'}), ('head_repository', {'full_name': 'fork/app'})]
        for key, value in variants:
            with self.subTest(key=key), self.assertRaises(ValueError):
                release.successful_run('123', {release.CONFIG['source_workflow']}, lambda _: {**self.run, key: value})
        for status in ('behind', 'diverged'):
            with self.subTest(status=status), self.assertRaises(ValueError):
                release.successful_run('123', {release.CONFIG['source_workflow']}, lambda path: {'status': status} if '/compare/' in path else self.run)

    def test_artifact_selection_rejects_ambiguity_expiry_and_missing_digests(self):
        self.assertEqual(release.unique_artifact(self.run, 'candidate', self.read), ('456', 'b' * 64))
        for artifacts in ([], [self.artifact, {**self.artifact, 'id': 457, 'name': 'candidate-123-2'}],
                          [{**self.artifact, 'expired': True}], [{**self.artifact, 'digest': ''}],
                          [{**self.artifact, 'workflow_run': {'id': 999}}]):
            with self.subTest(artifacts=artifacts), self.assertRaises(ValueError):
                release.unique_artifact(self.run, 'candidate', lambda _: {'artifacts': artifacts})

    def test_verification_failure_never_produces_upload_inputs(self):
        for error in ('digest mismatch', 'unapproved producer', 'wrong source', 'invalid signature'):
            with self.subTest(error=error), patch.object(release.fetch, 'candidate', side_effect=ValueError(error)):
                with self.assertRaisesRegex(ValueError, error):
                    release.resolve('Upload to TestFlight', '123', read=self.read)

    def test_verified_config_supplies_adapter_and_identity(self):
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as directory:
            try:
                os.chdir(directory)
                Path('release-assets').mkdir()
                Path('release-assets/release-build-manifest.json').write_text(json.dumps(self.manifest))
                Path('release-assets/app-config.json').write_text(json.dumps({'upload_adapter': 'transporter'}))
                with patch.object(release.fetch, 'candidate') as verify:
                    result = release.resolve('Upload to TestFlight', '123', read=self.read)
                verify.assert_called_once_with('456', 'b' * 64)
                self.assertEqual(result['upload_adapter'], 'transporter')
                self.assertEqual(result['candidate_id'], 'exact-candidate')
                self.assertEqual(result['processed_artifact_id'], '')
                self.assertEqual(result['operation'], 'testflight')
            finally:
                os.chdir(previous)

    def test_processed_resume_authenticates_both_handoffs_without_upload(self):
        promoted = {**self.run, 'id': 789, 'path': '.github/workflows/release.yml', 'event': 'workflow_dispatch', 'head_sha': 'd' * 40}
        final_artifact = {**self.artifact, 'id': 987, 'name': 'final-789-1', 'workflow_run': {'id': 789}}
        final = {**self.manifest, 'promotion': {'run_id': '789', 'run_attempt': '1', 'source_sha': 'd' * 40}}
        def read(path):
            if '/789/artifacts?' in path:
                return {'artifacts': [final_artifact]}
            if path.endswith('/789'):
                return promoted
            return self.read(path)
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as directory:
            try:
                os.chdir(directory)
                Path('release-assets').mkdir()
                Path('release-assets/release-build-manifest.json').write_text(json.dumps(self.manifest))
                Path('release-assets/app-config.json').write_text(json.dumps({'upload_adapter': 'transporter'}))
                for mode in ('valid', 'wrong-candidate', 'wrong-run', 'wrong-source', 'unapproved-signature'):
                    current = copy.deepcopy(final)
                    if mode == 'wrong-candidate': current['candidate_id'] = 'other'
                    if mode == 'wrong-run': current['promotion']['run_id'] = '999'
                    if mode == 'wrong-source': current['promotion']['source_sha'] = 'e' * 40
                    with self.subTest(mode=mode), patch.object(release.fetch, 'extract_artifact'), \
                            patch.object(release, 'verify', side_effect=ValueError('unapproved producer') if mode == 'unapproved-signature' else None, return_value=current), \
                            patch.object(release.fetch, 'candidate') as candidate, patch.object(release.fetch, 'processed') as processed:
                        if mode == 'valid':
                            result = release.resolve('Prepare App Store submission', '789', read=read)
                            self.assertEqual(result['processed_artifact_id'], '987')
                            self.assertEqual(result['resume_source'], f'https://github.com/{self.repo}/actions/runs/789')
                            candidate.assert_called_once_with('456', 'b' * 64)
                            processed.assert_called_once_with('987', 'b' * 64)
                        else:
                            with self.assertRaises(ValueError): release.resolve('Prepare App Store submission', '789', read=read)
                            processed.assert_not_called()
            finally:
                os.chdir(previous)

    def test_metadata_uses_reviewed_commit_and_keeps_existing_binary(self):
        with patch.object(release.fetch, 'release', return_value=self.manifest), patch.dict(os.environ, {'GITHUB_SHA': 'd' * 40}):
            result = release.resolve('Update store metadata', self.manifest['tag'], read=self.read)
            self.assertEqual(result['metadata_commit'], 'd' * 40)
            self.assertEqual(result['kind'], 'published')
            self.assertEqual(result['candidate_id'], self.manifest['candidate_id'])
            with self.assertRaises(ValueError): release.resolve('Update store metadata', self.manifest['tag'], 'main', read=self.read)
            with self.assertRaises(ValueError): release.resolve('Update store metadata', self.manifest['tag'], read=lambda _: {'status': 'diverged'})
            with self.assertRaises(ValueError): release.resolve('Upload to TestFlight', self.manifest['tag'], read=self.read)
        with self.assertRaises(ValueError): release.resolve('Update store metadata', '123', read=self.read)
        with self.assertRaises(ValueError): release.resolve('arbitrary action', '123', read=self.read)
