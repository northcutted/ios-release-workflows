import copy
import os
from pathlib import Path
import sys
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
os.environ.setdefault('IOS_RELEASE_CONFIG',str(Path(__file__).resolve().parents[3]/'examples/picstrip.json'))
from preflight import check_controls

class ControlTests(unittest.TestCase):
    def test_required_check_refs_and_bypass_drift_fail_closed(self):
        main={'id':1,'name':'Main','target':'branch','enforcement':'active','bypass_actors':[],
              'conditions':{'ref_name':{'include':['~DEFAULT_BRANCH'],'exclude':[]}},
              'rules':[{'type':t} for t in ('pull_request','deletion','non_fast_forward')]+[{'type':'required_status_checks','parameters':{'strict_required_status_checks_policy':True,'required_status_checks':[{'context':'CI Gate','integration_id':15368}]}}]}
        tags={'id':2,'name':'Release tags','target':'tag','enforcement':'active','bypass_actors':[{'actor_id':1,'actor_type':'Integration','bypass_mode':'always'}],
              'conditions':{'ref_name':{'include':['refs/tags/v*'],'exclude':[]}},'rules':[{'type':t} for t in ('creation','update','deletion')]}
        names=['signing','testflight','release-publishing','app-store-staging','production','app-store-observe']
        for failure in (None,'gate','main-bypass','tag-ref','environment-ref','approval','admin'):
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
                    with self.assertRaises(ValueError):check_controls(read)
                else:check_controls(read)
