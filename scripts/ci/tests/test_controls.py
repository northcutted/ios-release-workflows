import copy
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
os.environ.setdefault('IOS_RELEASE_CONFIG',str(Path(__file__).resolve().parents[3]/'examples/picstrip.json'))
from preflight import check_controls, verified_bypasses
from capture_controls import capture

class ControlTests(unittest.TestCase):
    def test_private_app_redaction_requires_unchanged_baseline_count_and_effective_permission(self):
        rule = {'id': 23700335, 'node_id': 'ruleset-node', 'name': 'Release tags',
                'updated_at': '2026-09-19T14:10:54.323Z', 'current_user_can_bypass': 'always'}
        actors = [{'actor_id': 3718913, 'actor_type': 'Integration', 'bypass_mode': 'always'}]
        baseline = dict(rule, bypass_actors=actors,
                        bypass_nodes=[{'id': 'owner-visible-private-integration', 'bypassMode': 'ALWAYS'}])
        # Actual administration:read installation response from the live canary.
        connection = {'totalCount': 1, 'pageInfo': {'hasNextPage': False}, 'nodes': [None]}
        self.assertEqual(actors, verified_bypasses(rule, baseline, lambda _: connection))
        for response in (dict(connection, nodes=[], totalCount=0), dict(connection, nodes=[None, None], totalCount=2),
                         dict(connection, nodes=[{'id': 'different', 'bypassMode': 'ALWAYS'}]),
                         dict(connection, pageInfo={'hasNextPage': True})):
            with self.subTest(response=response), self.assertRaises(ValueError):
                verified_bypasses(rule, baseline, lambda _: response)
        for change in ({'current_user_can_bypass': 'never'}, {'current_user_can_bypass': None},
                       {'updated_at': '2026-09-19T14:10:54.324Z'}, {'id': 999}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                verified_bypasses(dict(rule, **change), baseline, lambda _: connection)
        for bad in (dict(baseline, bypass_nodes=[]), dict(baseline, bypass_actors=[]),
                    dict(baseline, bypass_actors=[dict(actors[0], actor_type='RepositoryRole')])):
            with self.subTest(baseline=bad), self.assertRaises(ValueError):
                verified_bypasses(rule, bad, lambda _: connection)

    def test_ruleset_timestamp_compares_instants_without_losing_precision(self):
        rule = {'id': 1, 'node_id': 'node', 'name': 'Main', 'updated_at': '2026-09-19T14:10:53.839Z'}
        baseline = dict(rule, updated_at='2026-09-19T09:10:53.839-05:00', bypass_actors=[], bypass_nodes=[])
        connection = {'totalCount': 0, 'pageInfo': {'hasNextPage': False}, 'nodes': []}
        self.assertEqual([], verified_bypasses(rule, baseline, lambda _: connection))
        for timestamp in ('2026-09-19T14:10:53.840Z', '2026-09-19T14:10:53.839', None, ''):
            with self.subTest(timestamp=timestamp), self.assertRaises(ValueError):
                verified_bypasses(dict(rule, updated_at=timestamp), baseline, lambda _: connection)

    def test_hidden_bypasses_require_unchanged_owner_baseline_and_complete_readback(self):
        rule = {'id': 2, 'node_id': 'ruleset-node', 'name': 'Release tags', 'updated_at': '2026-09-19T14:10:54.323Z'}
        nodes = [{'id': 'private-app-bypass-node', 'bypassMode': 'ALWAYS'}]
        actors = [{'actor_id': 3718913, 'actor_type': 'Integration', 'bypass_mode': 'always'}]
        baseline = dict(rule, bypass_nodes=nodes, bypass_actors=actors)
        connection = {'totalCount': 1, 'pageInfo': {'hasNextPage': False}, 'nodes': nodes}
        self.assertEqual(actors, verified_bypasses(rule, baseline, lambda _: connection))
        for field in ('id', 'node_id', 'updated_at'):
            changed = dict(rule, **{field: 'changed'})
            with self.subTest(field=field), self.assertRaises(ValueError):
                verified_bypasses(changed, baseline, lambda _: connection)
        for response in (None, {}, dict(connection, totalCount=2),
                         dict(connection, pageInfo={'hasNextPage': True}),
                         dict(connection, nodes=[{'id': 'substituted', 'bypassMode': 'ALWAYS'}]),
                         dict(connection, nodes=[])):
            with self.subTest(response=response), self.assertRaises(ValueError):
                verified_bypasses(rule, baseline, lambda _: response)
        for bad_baseline in (None, {}, dict(baseline, bypass_actors=[])):
            with self.subTest(baseline=bad_baseline), self.assertRaises(ValueError):
                verified_bypasses(rule, bad_baseline, lambda _: connection)

    def test_owner_capture_rejects_redacted_or_racing_ruleset(self):
        base = {'id': 1, 'node_id': 'node', 'name': 'Main', 'updated_at': 'before'}
        def read(path):
            return [base] if path.endswith('/rulesets') else base
        with self.assertRaisesRegex(ValueError, 'owner session'):
            capture(1, read, lambda _: None)
        calls = []
        def racing(path):
            if path.endswith('/rulesets'):
                return [base]
            calls.append(path)
            return dict(base, bypass_actors=[], updated_at='before' if len(calls) == 1 else 'after')
        with self.assertRaisesRegex(ValueError, 'changed during capture'):
            capture(1, racing, lambda _: {'totalCount': 0, 'nodes': [], 'pageInfo': {'hasNextPage': False}})

    def test_required_check_refs_and_bypass_drift_fail_closed(self):
        main={'id':1,'name':'Main','target':'branch','enforcement':'active','bypass_actors':[],
              'conditions':{'ref_name':{'include':['~DEFAULT_BRANCH'],'exclude':[]}},
              'rules':[{'type':t} for t in ('pull_request','deletion','non_fast_forward')]+[{'type':'required_status_checks','parameters':{'strict_required_status_checks_policy':True,'required_status_checks':[{'context':'CI Gate','integration_id':15368}]}}]}
        tags={'id':2,'name':'Release tags','target':'tag','enforcement':'active','bypass_actors':[{'actor_id':1,'actor_type':'Integration','bypass_mode':'always'}],
              'conditions':{'ref_name':{'include':['refs/tags/v*'],'exclude':[]}},'rules':[{'type':t} for t in ('creation','update','deletion')]}
        names=['signing','testflight','release-publishing','app-store-staging','production','app-store-observe']
        for failure in (None,'gate','main-bypass','tag-ref','environment-ref','approval','admin','publisher'):
            m=copy.deepcopy(main);t=copy.deepcopy(tags)
            if failure=='gate':m['rules'][-1]['parameters']['required_status_checks'][0]['context']='Other'
            if failure=='main-bypass':m['bypass_actors']=[{'actor_type':'RepositoryRole'}]
            if failure=='tag-ref':t['conditions']['ref_name']['include']=['refs/tags/unrelated*']
            def read(path):
                tail=path.split('/picstrip')[-1]
                if tail=='':return {}
                if tail=='/environments':return {'environments':[{'name':n} for n in names]}
                if tail.endswith('/deployment-branch-policies'):
                    name=tail.split('/')[2]
                    return {'branch_policies':[{'name':'*' if failure=='environment-ref' else ('v*' if name in ('production','app-store-staging') else 'main'),'type':'tag' if name in ('production','app-store-staging') else 'branch'}]}
                if tail.startswith('/environments/'):
                    return {'can_admins_bypass':failure=='admin','deployment_branch_policy':{'custom_branch_policies':True},'protection_rules':[] if failure=='approval' else [{'type':'required_reviewers','reviewers':[{'id':1}]}]}
                if tail=='/immutable-releases':return {'enabled':True}
                if tail=='/rulesets':return [m,t]
                if tail=='/rulesets/1':return m
                if tail=='/rulesets/2':return t
                raise AssertionError(path)
            with self.subTest(failure=failure):
                if failure:
                    with self.assertRaises(ValueError):check_controls(read, controls={'publisher_app_id': 999} if failure=='publisher' else {})
                else:
                    check_controls(read)
                    with patch.dict(os.environ, {'IOS_RELEASE_CONTROL_POLICY': '{"publisher_app_id":999}'}):
                        with self.assertRaisesRegex(ValueError, 'Wrong publisher App'):
                            check_controls(read)
