import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {collect, generatedFiles, run, checkLinks} from '../../docs.mjs';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
function fixture(t, name = 'minimal') {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ios-docs-'));
  t.after(() => fs.rmSync(root, {recursive: true, force: true}));
  fs.mkdirSync(path.join(root, '.github/workflows'), {recursive: true});
  fs.mkdirSync(path.join(root, 'guide'));
  fs.symlinkSync(path.join(repo, 'node_modules'), path.join(root, 'node_modules'), 'dir');
  fs.copyFileSync(path.join(repo, 'package.json'), path.join(root, 'package.json'));
  fs.copyFileSync(path.join(repo, `examples/${name}.json`), path.join(root, '.github/ios-release.json'));
  fs.writeFileSync(path.join(root, '.github/ios-release-platform.json'), JSON.stringify({repository: 'northcutted/ios-release-workflows', revision: 'a'.repeat(40)}));
  fs.writeFileSync(path.join(root, '.github/ios-release-docs.json'), JSON.stringify({schema_version: 1, mode: 'consumer', output_dir: 'guide', pages: ['guide'], navigation: [{label: 'Guide', path: 'guide/index.md'}]}));
  fs.writeFileSync(path.join(root, 'guide/index.md'), '# Guide\n\n## Start\n');
  fs.writeFileSync(path.join(root, '.github/workflows/check.yml'), 'name: Check\non: push\njobs:\n  test:\n    runs-on: macos-26\n');
  return root;
}

test('unrelated apps generate deterministic docs with no PicStrip, classifier or Ruby pin requirement', t => {
  for (const name of ['minimal', 'extensions']) {
    const root = fixture(t, name);
    assert.deepEqual(run(root), []);
    const before = generatedFiles(root);
    assert.deepEqual(generatedFiles(root), before);
    assert.deepEqual(run(root, true), []);
    assert.ok(!before.get('guide/reference.json').includes('PicStrip'));
    const workflow = path.join(root, '.github/workflows/check.yml');
    fs.appendFileSync(workflow, '    timeout-minutes: 15\n');
    assert.equal(run(root, true).filter(error => error.includes('is stale')).length, 2);
    for (const [file, text] of before) assert.equal(fs.readFileSync(path.join(root, file), 'utf8'), text);
    assert.deepEqual(run(root), []);
    assert.deepEqual(run(root, true), []);
  }
});

test('reusable contracts retain false defaults, outputs, secrets and dependencies without executable steps', t => {
  const root = fixture(t);
  fs.writeFileSync(path.join(root, '.github/workflows/example.yaml'), `name: Example
on:
  workflow_call:
    inputs:
      submit:
        description: Stage | submit
        type: boolean
        default: false
    secrets:
      APPLE_KEY:
        required: true
    outputs:
      receipt:
        value: \${{ jobs.first.outputs.receipt }}
jobs:
  first:
    runs-on: ubuntu-24.04
    steps:
      - run: echo do-not-copy-step-code
  second:
    needs: first
    runs-on: ubuntu-24.04
    if: inputs.submit
`);
  const workflow = collect(root).workflows.find(workflow => workflow.name === 'Example');
  assert.equal(workflow.inputs.submit.default, false);
  assert.deepEqual(workflow.secret_names, ['APPLE_KEY']);
  assert.ok(workflow.outputs.receipt);
  assert.deepEqual(workflow.jobs[1].needs, ['first']);
  const files = generatedFiles(root);
  assert.ok(!files.get('guide/reference.json').includes('do-not-copy-step-code'));
  assert.match(files.get('guide/reference.md'), /Stage &#124; submit/);
  assert.deepEqual(run(root), []);
});

test('workspace configuration updates and malformed YAML rejects', t => {
  const root = fixture(t);
  const file = path.join(root, '.github/ios-release.json');
  const config = JSON.parse(fs.readFileSync(file, 'utf8'));
  config.workspace = 'Different.xcworkspace';
  delete config.project;
  config.locales.push('test-locale');
  fs.writeFileSync(file, JSON.stringify(config));
  assert.match(generatedFiles(root).get('guide/reference.md'), /Different.xcworkspace/);
  assert.equal(collect(root).configuration.expected_screenshots, config.locales.length * Object.keys(config.screenshot_classes).length * config.screens.length);
  fs.writeFileSync(path.join(root, '.github/workflows/broken.yml'), 'name: First\nname: Second\non: push\njobs: {}\n');
  assert.throws(() => collect(root), /unique/i);
});

test('configured pages validate links and headings while ignoring fenced examples', t => {
  const root = fixture(t);
  fs.mkdirSync(path.join(root, 'guide/nested'));
  fs.writeFileSync(path.join(root, 'guide/nested/page.md'), '# Page\n\n[Missing](missing.md)\n[Heading](../index.md#missing)\n[Good](../index.md#start)\n[External](https://example.invalid/)\n\n```md\n[Example](missing-example.md)\n```\n');
  assert.equal(checkLinks(root).length, 2);
});

test('platform mode documents its own interfaces and public command registry', () => {
  const data = collect(repo);
  assert.equal(data.configuration, null);
  assert.ok(data.commands.commands.qa);
  assert.ok(data.workflows.find(workflow => workflow.file.endsWith('/ci.yml')).inputs.source);
  assert.deepEqual(run(repo, true), []);
});
