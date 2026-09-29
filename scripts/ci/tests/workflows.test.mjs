import test from 'node:test';import assert from 'node:assert/strict';import {loadWorkflows,validate} from '../workflow_policy.mjs';
test('platform workflows enforce the declared boundaries',()=>assert.deepEqual(validate(loadWorkflows()),[]));
test('alternate submission, PR secrets, mutable actions and compilation OIDC fail policy',()=>{
 const w=loadWorkflows();w['deploy.yml'].jobs.review.environment='app-store-staging';w['ci.yml'].jobs.lint.steps.push({run:'echo bad',env:{KEY:'${{ secrets.KEY }}'}});w['prepare.yml'].jobs.build.steps[0].uses='actions/checkout@main';w['prepare.yml'].jobs.build.permissions={'id-token':'write'};
 const errors=validate(w).join('\n');for(const pattern of [/production approval/,/PR path/,/action must be pinned/,/compilation must not sign/])assert.match(errors,pattern);
});

test('environment secret declarations cannot silently disappear',()=>{const w=loadWorkflows();delete w['prepare.yml'].on.workflow_call.secrets.MATCH_SSH_PRIVATE_KEY;assert.match(validate(w).join('\n'),/explicit reusable contract/);});
test('release controller cannot bypass selection or choose a mutable artifact',()=>{
 const w=loadWorkflows();w['release.yml'].jobs.promote.needs=[];
 w['release.yml'].jobs.promote.with.artifact_id='latest';
 w['release.yml'].jobs.promote.with.upload_adapter='transporter';
 w['release.yml'].jobs.resolve.environment='testflight';
 const errors=validate(w).join('\n');
 assert.match(errors,/verified selection/);assert.match(errors,/frozen verified artifact/);
 assert.match(errors,/read-only and secret-free/);assert.match(errors,/verified candidate configuration/);
});

test('native QA cannot gain provenance credentials or reintroduce Fastlane',()=>{
 const w=loadWorkflows();w['ci.yml'].jobs.test.permissions={'id-token':'write'};
 w['ci.yml'].jobs.analyze.steps.push({uses:'$/actions/ruby'});
 const errors=validate(w).join('\n');assert.match(errors,/compilation must not sign/);assert.match(errors,/Native QA must not load/);
});
