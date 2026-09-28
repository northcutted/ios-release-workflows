import copy
import os
import json
from pathlib import Path
import sys
import unittest
import tempfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
os.environ.setdefault('IOS_RELEASE_CONFIG', str(Path(__file__).resolve().parents[3] / 'examples/picstrip.json'))
import observe
from observe import active, meaningful, eligible


class ObserveTests(unittest.TestCase):
    def setUp(self):
        self.record = {'candidate_id': 'candidate', 'build': {'id': 'build', 'attributes': {'processingState': 'VALID'}},
                       'version': {'id': 'version', 'attributes': {'versionString': '1.7.0', 'appVersionState': 'READY_FOR_DISTRIBUTION', 'releaseType': 'MANUAL'}},
                       'phased_release': None, 'observed_at': 'yesterday'}

    def test_timestamps_alone_do_not_count_as_state_changes(self):
        updated = copy.deepcopy(self.record); updated['observed_at'] = 'today'
        self.assertEqual(meaningful(updated), meaningful(self.record))
        for mutate in (lambda x: x['build'].update(id='other'),
                       lambda x: x['build']['attributes'].update(processingState='INVALID'),
                       lambda x: x['version']['attributes'].update(appVersionState='IN_REVIEW'),
                       lambda x: x.update(phased_release={'attributes': {'phasedReleaseState': 'ACTIVE', 'currentDayNumber': 2}})):
            updated = copy.deepcopy(self.record); mutate(updated)
            self.assertNotEqual(meaningful(updated), meaningful(self.record))

    def test_only_known_finished_states_are_inactive(self):
        self.assertFalse(active(self.record))
        for status in ('IN_REVIEW', 'PREPARE_FOR_SUBMISSION', 'REJECTED', 'UNKNOWN_FUTURE_STATE', None):
            updated = copy.deepcopy(self.record); updated['version']['attributes']['appVersionState'] = status
            self.assertTrue(active(updated))
        for phased in ('ACTIVE', 'PAUSED', 'UNKNOWN_FUTURE_STATE'):
            updated = copy.deepcopy(self.record); updated['phased_release'] = {'attributes': {'phasedReleaseState': phased}}
            self.assertTrue(active(updated))

    def test_only_immutable_release_evidence_is_observed(self):
        item = {'immutable': True, 'draft': False, 'prerelease': False, 'assets': [{'name': 'release-manifest.json'}]}
        self.assertEqual(eligible([item]), [item])
        for field, value in [('immutable', False), ('draft', True), ('prerelease', True), ('assets', [])]:
            self.assertEqual(eligible([{**item, field: value}]), [])

    def test_scheduled_poll_keeps_newest_and_active_releases_manual_refresh_checks_all(self):
        items = [{'tag_name': tag, 'immutable': True, 'draft': False, 'prerelease': False,
                  'assets': [{'name': 'release-manifest.json'}]} for tag in ['v1.7.0', 'v1.6.0', 'v1.5.0']]
        baseline = {item['tag_name']: {**copy.deepcopy(self.record), 'release_tag': item['tag_name']} for item in items}
        baseline['v1.5.0']['version']['attributes']['appVersionState'] = 'IN_REVIEW'
        previous_dir = Path.cwd()
        for event, expected in [('schedule', ['v1.7.0', 'v1.5.0']), ('workflow_dispatch', ['v1.7.0', 'v1.6.0', 'v1.5.0'])]:
            observed = []
            def authenticate(tag, mode):
                self.assertEqual(mode, 'observe')
                observed.append(tag)
                Path('release-assets').mkdir()
                Path('release-assets/app-config.json').write_text('{}')
            def apple_read(command, check):
                self.assertEqual(command[-1], 'observe')
                self.assertTrue(check)
                record = {**baseline[observed[-1]], 'observed_at': 'today'}
                Path(os.environ['OBSERVATION_RECEIPT']).write_text(json.dumps(record))
            with self.subTest(event=event), tempfile.TemporaryDirectory() as directory:
                try:
                    os.chdir(directory)
                    with patch.dict(os.environ, {'GITHUB_REPOSITORY': 'owner/app', 'GITHUB_EVENT_NAME': event,
                            'IOS_RELEASE_CONFIG': '/protected/config.json', 'IOS_RELEASE_ROOT': '/trusted/platform',
                            'GITHUB_STEP_SUMMARY': str(Path(directory) / 'summary.md')}), \
                            patch.object(observe, 'api', return_value=items), patch.object(observe, 'previous', return_value=baseline), \
                            patch.object(observe, 'release', side_effect=authenticate), patch.object(observe.subprocess, 'run', side_effect=apple_read):
                        observe.main()
                        self.assertEqual(os.environ['IOS_RELEASE_CONFIG'], '/protected/config.json')
                    self.assertEqual(observed, expected)
                    summary = json.loads(Path('build/observations/summary.json').read_text())
                    self.assertEqual(summary['changes'], [])
                    self.assertEqual(summary['retained_completed'], ['v1.6.0'] if event == 'schedule' else [])
                    self.assertIn('No App Store state changes', Path('summary.md').read_text())
                finally:
                    os.chdir(previous_dir)

    def test_broken_previous_snapshot_falls_back_to_full_observation(self):
        with patch.dict(os.environ, {'GITHUB_REPOSITORY': 'owner/app', 'GITHUB_RUN_ID': '123',
                                     'GITHUB_WORKFLOW_REF': 'owner/app/.github/workflows/observe.yml@refs/heads/main'}):
            self.assertEqual(observe.previous(lambda _: {'malformed': []}), {})
