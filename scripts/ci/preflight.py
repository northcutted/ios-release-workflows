"""Fail closed when required live release controls drift."""
from fetch import api
from configuration import CONFIG, require


def check_controls(read=api):
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
    require(main is not None and not main['bypass_actors'], 'Missing main rules or bypass actors enabled')
    refs = main['conditions']['ref_name']
    require(refs['include'] in (['~DEFAULT_BRANCH'], ['refs/heads/main']) and not refs['exclude'], 'Main rule applies to wrong refs')
    controls = {r['type']: r.get('parameters', {}) for r in main['rules']}
    require({'pull_request', 'required_status_checks', 'deletion', 'non_fast_forward'} <= set(controls), 'Missing main controls')
    checks = controls['required_status_checks']
    require(checks.get('strict_required_status_checks_policy') and {'context':'CI Gate','integration_id':15368} in checks['required_status_checks'], 'CI Gate from GitHub Actions must be required and up to date')
    tags = next((r for r in rules if r['target'] == 'tag' and r['name'] == 'Release tags'), None)
    require(tags is not None and {'creation', 'update', 'deletion'} <= {r['type'] for r in tags['rules']}, 'Missing release-tag controls')
    require(tags['conditions']['ref_name'] == {'include':['refs/tags/v*'],'exclude':[]}, 'Release-tag protection applies to wrong refs')
    actors = tags['bypass_actors']
    require(len(actors) == 1 and actors[0]['actor_type'] == 'Integration' and actors[0]['bypass_mode'] == 'always', 'Only the publisher App may create release tags')
    print('Required branch, tag, environment, approval, and immutable-release controls passed.')


if __name__ == '__main__':
    check_controls()
