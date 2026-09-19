"""Fail closed when repository controls required by the release platform are absent."""
from fetch import api
from configuration import CONFIG, require

prefix = f"repos/{CONFIG['repository']}"
repo = api(prefix)
environments = {e['name']: e for e in api(prefix + '/environments')['environments']}
required = {'signing', 'testflight', 'release-publishing', 'app-store-staging', 'production', 'app-store-observe'}
require(required <= set(environments), 'Missing release environments')
production = api(prefix + '/environments/production')
require(production.get('can_admins_bypass') is False, 'Production approval bypass is enabled')
require(any(r['type'] == 'required_reviewers' and r.get('reviewers') for r in production['protection_rules']), 'Production approval is required; private repositories need Enterprise Cloud')
for name in required:
    env = environments[name]
    require((env.get('deployment_branch_policy') or {}).get('custom_branch_policies'), f'Unrestricted environment: {name}')
require(api(prefix + '/immutable-releases')['enabled'], 'Immutable releases are disabled')
rules = [api(prefix + '/rulesets/' + str(r['id'])) for r in api(prefix + '/rulesets') if r['enforcement'] == 'active']
main = next((r for r in rules if r['target'] == 'branch' and r['name'] == 'Main'), None)
require(main is not None and {'pull_request', 'required_status_checks'} <= {r['type'] for r in main['rules']}, 'Missing required main PR/check controls')
tags = next((r for r in rules if r['target'] == 'tag' and r['name'] == 'Release tags'), None)
require(tags is not None and {'creation', 'update', 'deletion'} <= {r['type'] for r in tags['rules']}, 'Missing release-tag controls')
print('Release control preflight passed. Native attestation generation also enforces GitHub plan availability.')
