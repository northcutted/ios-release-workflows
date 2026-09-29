#!/usr/bin/env node
// Local, deterministic reference generation. No GitHub/Apple requests, credentials,
// workflow execution, or platform checkout are needed. Prose explains intent;
// this projection records the checked-in consumer contract.
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';

const repo = process.env.IOS_APP_ROOT || process.cwd();
const profilePath = '.github/ios-release-docs.json';
const ownRoot = fileURLToPath(new URL('../', import.meta.url));

function profile(root) {
  const data = readJSON(root, profilePath);
  if (data.schema_version !== 1) throw new Error('Unsupported documentation profile');
  for (const value of [data.output_dir, ...data.pages, ...data.navigation.map(link => link.path), ...(data.classifier ? [data.classifier] : [])]) {
    if (!value || path.isAbsolute(value) || value.split('/').includes('..')) throw new Error('Documentation paths must stay inside the repository');
  }
  return data;
}
const readJSON = (root, file) => JSON.parse(fs.readFileSync(path.join(root, file), 'utf8'));
const array = value => value == null ? [] : Array.isArray(value) ? value : [value];
const pick = (object, keys) => Object.fromEntries(keys.filter(key => key in object).map(key => [key, object[key]]));

export function collect(root = repo) {
  const settings = profile(root);
  const YAML = createRequire(path.join(root, 'package.json'))('yaml');
  const config = settings.mode === 'platform' ? null : readJSON(root, '.github/ios-release.json');
  const platform = config ? readJSON(root, '.github/ios-release-platform.json') : {repository: settings.repository};
  const workflows = fs.readdirSync(path.join(root, '.github/workflows')).sort()
    .filter(file => /\.ya?ml$/.test(file)).map(file => {
      const source = `.github/workflows/${file}`;
      const workflow = YAML.parse(fs.readFileSync(path.join(root, source), 'utf8'), {uniqueKeys: true});
      if (!workflow?.name || !workflow.on || !workflow.jobs) throw new Error(`Incomplete workflow: ${source}`);
      const events = typeof workflow.on === 'string' || Array.isArray(workflow.on)
        ? Object.fromEntries(array(workflow.on).map(event => [event, null])) : workflow.on;
      return {
        file: source, name: workflow.name,
        triggers: Object.fromEntries(Object.entries(events).map(([event, value]) => [event,
          ['workflow_dispatch', 'workflow_call'].includes(event) ? {} : value])),
        inputs: events.workflow_call?.inputs ?? events.workflow_dispatch?.inputs ?? {},
        secret_names: Object.keys(events.workflow_call?.secrets ?? {}).sort(),
        outputs: events.workflow_call?.outputs ?? {},
        permissions: workflow.permissions ?? null,
        concurrency: workflow.concurrency ?? null,
        jobs: Object.entries(workflow.jobs).map(([id, job]) => ({
          id, name: job.name ?? id, needs: array(job.needs),
          condition: job.if ?? null, runner: job['runs-on'] ?? null,
          uses: job.uses ?? null, environment: job.environment ?? null,
          timeout_minutes: job['timeout-minutes'] ?? null,
          permissions: job.permissions ?? null,
          secret_names: typeof job.secrets === 'object' ? Object.keys(job.secrets).sort() : [],
        })),
      };
    });
  return {
    schema_version: 2,
    scope: 'Checked-in workflow contracts; excludes live service state and step scripts.',
    sources: [profilePath, ...(config ? ['.github/ios-release.json', '.github/ios-release-platform.json'] : []),
      ...(fs.existsSync(path.join(root, '.ruby-version')) ? ['.ruby-version'] : []),
      ...(settings.classifier ? [settings.classifier] : []), ...(config ? [] : ['scripts/cli.py']), ...workflows.map(workflow => workflow.file)],
    platform, settings,
    configuration: config ? {
      ...pick(config, ['repository', 'default_branch', 'project', 'workspace', 'scheme', 'xcode', 'compatibility',
        'test_device', 'test_targets', 'qa_checks', 'upload_adapter', 'metadata_path', 'screenshots_path',
        'locales', 'localization_locales', 'screenshot_devices', 'screens', 'replacement_release']),
      app_store: pick(config.app_store, ['release_type', 'phased_release', 'testflight_groups']),
      developer_ruby: fs.existsSync(path.join(root, '.ruby-version')) ? fs.readFileSync(path.join(root, '.ruby-version'), 'utf8').trim() : null,
      expected_screenshots: config.locales.length * Object.keys(config.screenshot_classes).length * config.screens.length,
    } : null,
    // This command runs only in local/unprivileged documentation checks. It never
    // participates in trusted release verification or imports app code there.
    change_examples: settings.classifier ? JSON.parse(execFileSync(process.execPath,
      [path.join(root, settings.classifier), 'examples', JSON.stringify(settings.change_examples)],
      {cwd: root, encoding: 'utf8'})) : [],
    commands: config ? null : JSON.parse(execFileSync('python3',
      [path.join(ownRoot, 'bin/ios-release'), '--commands-json'], {encoding: 'utf8'})),
    workflows,
  };
}

const escape = value => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;')
  .replaceAll('>', '&gt;').replaceAll('|', '&#124;').replaceAll('`', '&#96;').replaceAll('\n', ' ');
const code = value => `<code>${escape(typeof value === 'object' ? JSON.stringify(value) : value)}</code>`;
const table = (headings, rows) => [headings, headings.map(() => '---'), ...rows]
  .map(row => `| ${row.join(' | ')} |`).join('\n');
const anchor = value => value.toLowerCase().replace(/[^\p{L}\p{N}_\-\s]/gu, '').replace(/\s/g, '-');
const callLink = (uses, data) => {
  if (uses.startsWith('$/')) return `[${uses.slice(2)}](${path.posix.relative(data.settings.output_dir, uses.slice(2))})`;
  const [target, revision] = uses.split('@');
  const [owner, name, ...file] = target.split('/');
  return `[${file.at(-1)}](https://github.com/${owner}/${name}/blob/${revision}/${file.join('/')})`;
};

export function renderMarkdown(data) {
  const config = data.configuration;
  const local = file => path.posix.relative(data.settings.output_dir, file);
  const navigation = data.settings.navigation.map(link => `[${link.label}](${local(link.path)})`).join(' · ');
  const lines = ['# CI/CD reference', '',
    '<!-- Generated by the pinned ios-release platform. Edit sources, then run the documented docs command. -->', '',
    navigation, '',
    'Generated from checked-in workflow interfaces and configuration. This reference records declared contracts; inspect live services for current state. Step scripts and secret values are omitted.', '',
    `[Machine-readable index](reference.json) (schema ${data.schema_version}). Regenerate with \`make docs\`; verify with \`make check-docs\`.`, '',
    ...(data.platform.revision ? ['## Platform guides', '', ...['setup', 'operations', 'architecture', 'maintenance'].map(name => `[${name[0].toUpperCase() + name.slice(1)}](https://github.com/${data.platform.repository}/blob/${data.platform.revision}/docs/${name}.md)`), '',] : []),
    '## Platform and configuration', '',
    data.platform.revision ? `Platform: [${data.platform.repository} at ${data.platform.revision.slice(0, 12)}](https://github.com/${data.platform.repository}/tree/${data.platform.revision}).` : `Platform: ${data.platform.repository}.` , '',
    ...(config ? [
    `Source: [app configuration](${local('.github/ios-release.json')}), [platform pin](${local('.github/ios-release-platform.json')}).`, '',
    table(['Setting', 'Checked-in value'], [
      ['Project / scheme', `${code(config.workspace || config.project)} / ${code(config.scheme)}`],
      ['Primary Xcode / build / simulator / SDK', [config.xcode.version, config.xcode.build, config.xcode.runtime, config.xcode.sdk].map(code).join(' / ')],
      ['Compatibility Xcode / build / simulator / SDK', [config.compatibility.version, config.compatibility.build, config.compatibility.runtime, config.compatibility.sdk].map(code).join(' / ')],
      ['Unit test device', escape(config.test_device)],
      ['Developer Ruby', code(config.developer_ruby)],
      ['Release QA checks', config.qa_checks.map(code).join(', ')],
      ['Upload adapter', code(config.upload_adapter)],
      ['Store release policy', `${code(config.app_store.release_type)}; phased release: ${code(config.app_store.phased_release)}`],
      ['Automatic TestFlight groups', config.app_store.testflight_groups.length ? config.app_store.testflight_groups.map(escape).join(', ') : 'None configured'],
      ['Replacement override', config.replacement_release ? `${code(config.replacement_release.version)} replaces recorded build ${code(config.replacement_release.build_number)}` : 'None'],
      ['Store locales', `${config.locales.length}: ${config.locales.map(code).join(', ')}`],
      ['App translations', `${config.localization_locales.length} in addition to the source language`],
      ['Screenshot devices', config.screenshot_devices.map(escape).join(', ')],
      ['Screenshot inventory', `${config.expected_screenshots} images; ${config.screens.length} scenes per device class and store locale`],
      ['Screenshot scenes', config.screens.map(code).join(', ')],
    ]), '',
    ...(data.change_examples.length ? ['## Which checks run?', '',
    'Single-path examples evaluated by the consumer classifier. Mixed changes and fallback behavior follow that source.', '',
    table(['Changed path', ...Object.values(data.settings.example_flags)], data.change_examples.map(example =>
      [code(example.path), ...Object.keys(data.settings.example_flags).map(flag => example[flag] ? 'Yes' : '—')])), '',
    ] : []),

    ] : []),
    ...(data.commands ? ['## Commands', '', table(['Command', 'Purpose'], Object.entries(data.commands.commands).map(([name, command]) => [code(name), escape(command.description)])), ''] : []),
    '## Workflows', '',
    table(['Workflow', 'File', 'Events'], data.workflows.map(workflow => [
      `[${escape(workflow.name)}](#${anchor(workflow.name)})`, `[${workflow.file.split('/').at(-1)}](${local(workflow.file)})`,
      Object.keys(workflow.triggers).map(code).join(', ')])), '',
    'Jobs below belong to the checked-in workflows. A linked reusable workflow expands into its own jobs. A dash means no explicit override; GitHub dependency/default behavior still applies. Conditions are shown verbatim, not evaluated here.', '',
  ];
  for (const workflow of data.workflows) {
    lines.push(`### ${workflow.name}`, '',
      `[Source](${local(workflow.file)}) · [Actions](https://github.com/${config?.repository || data.platform.repository}/actions/workflows/${workflow.file.split('/').at(-1)})`, '',
      'Triggers (cron expressions use UTC):', '', '```json', JSON.stringify(workflow.triggers, null, 2), '```', '',
      `Concurrency: ${code(workflow.concurrency)}. Default token permissions: ${code(workflow.permissions)}.`, '');
    if (Object.keys(workflow.inputs).length) lines.push(
      table(['Input', 'Type', 'Required', 'Default', 'Description / choices'], Object.entries(workflow.inputs).map(([name, input]) => [
        code(name), code(input.type ?? 'string'), input.required ? 'Yes' : 'No',
        Object.hasOwn(input, 'default') ? code(input.default === '' ? '"" (blank)' : input.default) : '—',
        [escape(input.description ?? ''), input.options?.map(code).join(', ')].filter(Boolean).join('<br>'),
      ])), '');
    if (workflow.secret_names.length) lines.push(`Named secrets: ${workflow.secret_names.map(code).join(', ')}.`, '');
    if (Object.keys(workflow.outputs).length) lines.push('Outputs:', '', '```json', JSON.stringify(workflow.outputs, null, 2), '```', '');
    lines.push(table(['Job', 'Needs', 'Execution', 'Condition'], workflow.jobs.map(job => [
      `${code(job.id)}${job.name !== job.id ? `<br>${escape(job.name)}` : ''}`,
      job.needs.length ? job.needs.map(code).join(', ') : '—',
      job.uses ? callLink(job.uses, data) : `${code(job.runner)}; ${job.timeout_minutes ?? 'default'} min${job.environment ? `<br>Environment: ${code(job.environment)}` : ''}`,
      job.condition == null ? '—' : code(job.condition),
    ])), '');
  }
  return lines.join('\n');
}

export function generatedFiles(root = repo) {
  const data = collect(root);
  const outputDir = data.settings.output_dir;
  return new Map([
    [`${outputDir}/reference.json`, JSON.stringify(data, null, 2) + '\n'],
    [`${outputDir}/reference.md`, renderMarkdown(data)],
  ]);
}

// These are simple repo-owned Markdown links, not a general Markdown parser.
// Scan every maintained CI/CD page; historical reviews/evidence are not rewritten.
function markdownFiles(directory) {
  return fs.readdirSync(directory, {withFileTypes: true}).sort((a, b) => a.name.localeCompare(b.name))
    .flatMap(entry => entry.isDirectory() ? markdownFiles(path.join(directory, entry.name))
      : entry.name.endsWith('.md') ? [path.join(directory, entry.name)] : []);
}
function withoutFences(text) {
  return text.replace(/^(`{3,}|~{3,})[^\n]*\n[\s\S]*?^\1\s*$/gm, '');
}
export function checkLinks(root = repo) {
  const errors = [];
  const pages = profile(root).pages.flatMap(page => {
    const target = path.join(root, page);
    return fs.statSync(target).isDirectory() ? markdownFiles(target) : [target];
  });
  for (const file of pages) {
    const content = withoutFences(fs.readFileSync(file, 'utf8'));
    for (const match of content.matchAll(/\[[^\]\n]*\]\(([^\s)]+)\)/g)) {
      const href = match[1];
      if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('//')) continue;
      const [relative, fragment] = href.split('#');
      const target = relative ? path.resolve(path.dirname(file), decodeURIComponent(relative)) : file;
      if (!fs.existsSync(target)) { errors.push(`${path.relative(root, file)}: missing link ${href}`); continue; }
      if (fragment && target.endsWith('.md')) {
        const headings = [...withoutFences(fs.readFileSync(target, 'utf8')).matchAll(/^#{1,6}\s+(.+)$/gm)].map(match => anchor(match[1]));
        if (!headings.includes(decodeURIComponent(fragment))) errors.push(`${path.relative(root, file)}: missing heading ${href}`);
      }
    }
  }
  return errors;
}

export function run(root = repo, check = false) {
  const errors = [];
  for (const [file, contents] of generatedFiles(root)) {
    const target = path.join(root, file);
    if (check) {
      if (!fs.existsSync(target) || fs.readFileSync(target, 'utf8') !== contents) errors.push(`${file} is stale; regenerate with the documented docs command`);
    } else {
      fs.mkdirSync(path.dirname(target), {recursive: true});
      if (!fs.existsSync(target) || fs.readFileSync(target, 'utf8') !== contents) fs.writeFileSync(target, contents);
    }
  }
  return [...errors, ...checkLinks(root)];
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  if (args.length > 1 || args.some(arg => arg !== '--check')) {
    console.error('Usage: ios-release docs [--check]');
    process.exitCode = 1;
  } else {
    try {
      const errors = run(repo, args.includes('--check'));
      if (errors.length) throw new Error(errors.join('\n'));
      console.log(args.includes('--check') ? 'CI/CD reference is current; local documentation links passed.' : 'CI/CD reference generated; local documentation links passed.');
    } catch (error) { console.error(error.message); process.exitCode = 1; }
  }
}
