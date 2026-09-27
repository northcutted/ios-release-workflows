import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import {analyze} from '../../semantic_dry_run.mjs';

function repository(t) {
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), 'picstrip-version-'));
  t.after(() => fs.rmSync(cwd, {recursive: true, force: true}));
  const git = (...args) => execFileSync('git', args, {cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe']}).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'CI Test'); git('config', 'user.email', 'ci@example.invalid');
  fs.copyFileSync('.releaserc.json', path.join(cwd, '.releaserc.json'));
  git('add', '.'); git('commit', '-m', 'chore: initial'); git('tag', 'v1.6.5');
  return {cwd, git, commit: message => git('commit', '--allow-empty', '-m', message)};
}

test('read-only analysis preserves release rules and does not tag or dirty source', async t => {
  const {cwd, git, commit} = repository(t);
  commit('docs: explain usage'); assert.equal((await analyze(cwd)).will_release, false);
  commit('fix: reject corrupt images');
  const patch = await analyze(cwd);
  assert.equal(patch.version, '1.6.6');
  assert.match(patch.notes, /Bug Fixes/);
  assert.match(patch.notes, /reject corrupt images/);
  assert.match(patch.notes, /\/commit\/[a-f0-9]{40}/);
  commit('feat: add iOS 27 support'); assert.equal((await analyze(cwd)).version, '1.7.0');
  commit('feat!: change export contract');
  const major = await analyze(cwd);
  assert.equal(major.version, '2.0.0');
  assert.match(major.notes, /BREAKING CHANGE/);
  assert.match(major.notes, /change export contract/);
  assert.equal(git('tag'), 'v1.6.5'); assert.equal(git('status', '--porcelain'), '');
});
test('ignore unrelated tags and use highest reachable stable semantic version', async t => {
  const {cwd, git, commit} = repository(t);
  git('checkout', '-b', 'unrelated'); commit('feat: other branch'); git('tag', 'v99.0.0'); git('checkout', 'main');
  git('tag', 'v2.0.0-beta.1'); git('tag', `v9.0.0-deploy-${'a'.repeat(40)}`); commit('perf: accelerate scan');
  assert.equal((await analyze(cwd)).version, '1.6.6');
});
test('a reverted change alone does not create a release', async t => {
  const {cwd, git, commit} = repository(t);
  commit('feat: temporary feature'); const hash = git('rev-parse', 'HEAD');
  commit(`Revert "feat: temporary feature"\n\nThis reverts commit ${hash}.`);
  assert.equal((await analyze(cwd)).will_release, false);
});

test('replacement keeps the highest released marketing version without moving its tag', async t => {
  const {cwd, git, commit} = repository(t);
  commit('feat: revise privacy review');
  fs.writeFileSync(path.join(cwd, 'replacement.json'), JSON.stringify({replacement_release: {version: '1.6.5', source_tag: 'v1.6.5'}}));
  const before = git('rev-parse', 'v1.6.5');
  const result = await analyze(cwd, 'replacement.json');
  assert.equal(result.version, '1.6.5');
  assert.equal(result.replacement_for, 'v1.6.5');
  assert.equal(result.git_tag, null, 'The build job adds its exact build number later');
  assert.equal(result.will_release, true);
  assert.equal(git('rev-parse', 'v1.6.5'), before);
  assert.equal(git('tag'), 'v1.6.5');
});
test('replacement refuses missing, older, mismatched, or prerelease version tags', async t => {
  const {cwd} = repository(t);
  for (const [version, source_tag] of [['1.5.0', 'v1.5.0'], ['1.7.0', 'v1.7.0'], ['1.6.5', 'v1.5.0'], ['1.6.5-beta.1', 'v1.6.5-beta.1']]) {
    fs.writeFileSync(path.join(cwd, 'replacement.json'), JSON.stringify({replacement_release: {version, source_tag}}));
    await assert.rejects(analyze(cwd, 'replacement.json'), /highest reachable stable release/);
  }
});
