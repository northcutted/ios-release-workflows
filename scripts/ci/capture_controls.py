#!/usr/bin/env python3
"""Record an owner-visible bypass baseline without changing repository settings."""
import argparse
import json
from pathlib import Path
from configuration import CONFIG, require
from fetch import api
from preflight import bypass_nodes, check_controls


def capture(publisher_app_id, read=api, graphql=bypass_nodes):
    prefix = f"repos/{CONFIG['repository']}/rulesets"
    snapshots = {}
    for rule in read(prefix):
        if rule['name'] not in ('Main', 'Release tags'):
            continue
        before = read(prefix + '/' + str(rule['id']))
        require('bypass_actors' in before, 'Use an owner session that can inspect ruleset bypass actors')
        connection = graphql(before['node_id'])
        require(connection and connection['pageInfo']['hasNextPage'] is False and
                connection['totalCount'] == len(connection['nodes']) == len(before['bypass_actors']),
                'Incomplete bypass readback')
        after = read(prefix + '/' + str(rule['id']))
        require(before == after, 'Ruleset changed during capture; retry')
        snapshots[rule['name']] = {key: before[key] for key in ('id', 'node_id', 'updated_at', 'bypass_actors')}
        snapshots[rule['name']]['bypass_nodes'] = connection['nodes']
    controls = {'publisher_app_id': publisher_app_id, 'rulesets': snapshots}
    check_controls(read, graphql, controls)
    require(set(snapshots) == {'Main', 'Release tags'}, 'Missing required ruleset snapshots')
    return controls


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publisher-app-id', type=int, required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    require(args.publisher_app_id > 0, 'Invalid publisher App ID')
    Path(args.output).write_text(json.dumps(capture(args.publisher_app_id), indent=2) + '\n')
