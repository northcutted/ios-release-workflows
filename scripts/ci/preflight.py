"""Fail closed when required live release controls drift."""
from fetch import api
from configuration import CONFIG, require
import json
import subprocess


def bypass_nodes(node_id):
    query = '''query($id: ID!) { node(id: $id) { ... on RepositoryRuleset {
      id bypassActors(first: 100) { totalCount pageInfo { hasNextPage }
        nodes { id bypassMode } }
    } } }'''
    result = json.loads(subprocess.check_output(
        ['gh', 'api', 'graphql', '-f', 'query=' + query, '-f', 'id=' + node_id], text=True))
    require(not result.get('errors'), 'Unable to inspect ruleset bypass actors')
    node = result.get('data', {}).get('node')
    require(node and node.get('id') == node_id, 'Wrong or inaccessible ruleset node')
    return node.get('bypassActors')


def verified_bypasses(rule, baseline, graphql=bypass_nodes):
    # REST hides bypass actors from administration:read. Never interpret omission
    # as an empty list or give the publisher permission to edit its own guardrails.
    if 'bypass_actors' in rule:
        return rule['bypass_actors']
    require(isinstance(baseline, dict), 'Hidden bypass actors require an owner-recorded controls baseline')
    for key in ('id', 'node_id', 'updated_at'):
        require(rule.get(key) and rule[key] == baseline.get(key), 'Ruleset changed since owner verification: ' + rule['name'])
    connection = graphql(rule['node_id'])
    require(isinstance(connection, dict) and isinstance(connection.get('nodes'), list), 'Incomplete bypass readback')
    nodes = connection['nodes']
    require(connection.get('pageInfo', {}).get('hasNextPage') is False and
            connection.get('totalCount') == len(nodes), 'Truncated bypass readback')
    require(nodes == baseline.get('bypass_nodes'), 'Ruleset bypass identities changed')
    actors = baseline.get('bypass_actors')
    require(isinstance(actors, list) and len(actors) == len(nodes), 'Invalid controls baseline')
    return actors


def check_controls(read=api, graphql=bypass_nodes, controls=None):
    controls = CONFIG.get('github_controls', {}) if controls is None else controls
    prefix = f"repos/{CONFIG['repository']}"
    read(prefix)
    environments = {e['name']: e for e in read(prefix + '/environments')['environments']}
    expected = {'signing': ('main', 'branch'), 'testflight': ('main', 'branch'),
                'release-publishing': ('main', 'branch'), 'app-store-staging': ('v*', 'tag'),
                'production': ('v*', 'tag'), 'app-store-observe': ('main', 'branch')}
    require(set(expected) <= set(environments), 'Missing release environments')
    for name, ref in expected.items():
        env = read(prefix + '/environments/' + name)
        require(env.get('can_admins_bypass') is False, f'Approval bypass enabled: {name}')
        require((env.get('deployment_branch_policy') or {}).get('custom_branch_policies'), f'Unrestricted environment: {name}')
        policies = read(prefix + f'/environments/{name}/deployment-branch-policies')['branch_policies']
        require([(p['name'], p['type']) for p in policies] == [ref], f'Wrong eligible refs: {name}')
        if name == 'production':
            require(any(r['type'] == 'required_reviewers' and r.get('reviewers') for r in env['protection_rules']), 'Production approval is required; private repositories need Enterprise Cloud')
    require(read(prefix + '/immutable-releases')['enabled'], 'Immutable releases are disabled')
    rules = [read(prefix + '/rulesets/' + str(r['id'])) for r in read(prefix + '/rulesets') if r['enforcement'] == 'active']
    main = next((r for r in rules if r['target'] == 'branch' and r['name'] == 'Main'), None)
    require(main is not None, 'Missing main rules')
    require(not verified_bypasses(main, controls.get('rulesets', {}).get('Main'), graphql), 'Main bypass actors enabled')
    refs = main['conditions']['ref_name']
    require(refs['include'] in (['~DEFAULT_BRANCH'], ['refs/heads/main']) and not refs['exclude'], 'Main rule applies to wrong refs')
    protections = {r['type']: r.get('parameters', {}) for r in main['rules']}
    require({'pull_request', 'required_status_checks', 'deletion', 'non_fast_forward'} <= set(protections), 'Missing main controls')
    checks = protections['required_status_checks']
    require(checks.get('strict_required_status_checks_policy') and {'context':'CI Gate','integration_id':15368} in checks['required_status_checks'], 'CI Gate from GitHub Actions must be required and up to date')
    tags = next((r for r in rules if r['target'] == 'tag' and r['name'] == 'Release tags'), None)
    require(tags is not None and {'creation', 'update', 'deletion'} <= {r['type'] for r in tags['rules']}, 'Missing release-tag controls')
    require(tags['conditions']['ref_name'] == {'include':['refs/tags/v*'],'exclude':[]}, 'Release-tag protection applies to wrong refs')
    actors = verified_bypasses(tags, controls.get('rulesets', {}).get('Release tags'), graphql)
    require(len(actors) == 1 and actors[0]['actor_type'] == 'Integration' and actors[0]['bypass_mode'] == 'always', 'Only the publisher App may create release tags')
    if controls:
        require(actors[0]['actor_id'] == controls.get('publisher_app_id'), 'Wrong publisher App')
    print('Required branch, tag, environment, approval, and immutable-release controls passed.')


if __name__ == '__main__':
    check_controls()
