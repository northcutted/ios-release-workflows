import test from 'node:test';import assert from 'node:assert/strict';import {loadWorkflows,validate} from '../workflow_policy.mjs';
test('platform workflows enforce the declared boundaries',()=>assert.deepEqual(validate(loadWorkflows()),[]));
test('alternate submission, PR secrets, mutable actions and compilation OIDC fail policy',()=>{
 const w=loadWorkflows();w['deploy.yml'].jobs.review.environment='app-store-staging';w['ci.yml'].jobs.lint.steps.push({run:'echo bad',env:{KEY:'${{ secrets.KEY }}'}});w['prepare.yml'].jobs.build.steps[0].uses='actions/checkout@main';w['prepare.yml'].jobs.build.permissions={'id-token':'write'};
 const errors=validate(w).join('\n');for(const pattern of [/production approval/,/PR path/,/action must be pinned/,/compilation must not sign/])assert.match(errors,pattern);
});

test('environment secret declarations cannot silently disappear',()=>{const w=loadWorkflows();delete w['prepare.yml'].on.workflow_call.secrets.MATCH_SSH_PRIVATE_KEY;assert.match(validate(w).join('\n'),/explicit reusable contract/);});
