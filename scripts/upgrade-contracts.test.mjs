import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { assertRequiredJobs } from './ci-aggregate.mjs';

const root = new URL('../', import.meta.url);
const read = path => readFileSync(new URL(path, root), 'utf8');

test('UP-T002 a behavioral mutation of the shipped input helper fails real regression assertions', () => {
  const directory = mkdtempSync(join(tmpdir(), 'connector-helper-mutation-'));
  try {
    mkdirSync(join(directory, 'tests'));
    mkdirSync(join(directory, 'js'));
    writeFileSync(join(directory, 'tests/input.test.cjs'), read('plugin/tests/input.test.cjs'));
    const helper = read('plugin/js/input.js');
    writeFileSync(join(directory, 'js/input.js'), helper);
    const env = { ...process.env };
    delete env.NODE_TEST_CONTEXT;
    const execute = () => spawnSync(process.execPath, ['--test', join(directory, 'tests/input.test.cjs')], { encoding: 'utf8', env });
    assert.equal(execute().status, 0, 'The unmodified shipped helper must pass the copied original test');
    const mutant = helper.replace('expected = text;', 'expected = String(el.value) + text;');
    assert.notEqual(mutant, helper, 'Mutation must match a real production behavior');
    writeFileSync(join(directory, 'js/input.js'), mutant);
    const failed = execute();
    assert.equal(failed.status, 1, 'Appending instead of filling must fail the unchanged tests');
    assert.match(failed.stdout + failed.stderr, /fill replaces rather than appends/u);
    assert.doesNotMatch(failed.stdout + failed.stderr, /SyntaxError/u);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('UP-T003 test dependencies are exact, locked and developer-path independent', () => {
  const pkg = JSON.parse(read('package.json'));
  const lock = JSON.parse(read('package-lock.json'));
  for (const [name, version] of Object.entries(pkg.devDependencies)) {
    assert.match(version, /^\d+\.\d+\.\d+$/u);
    assert.equal(lock.packages[`node_modules/${name}`].version, version);
    assert.ok(lock.packages[`node_modules/${name}`].integrity);
  }
  for (const path of ['plugin/tests/workflow/page.test.cjs', 'examples/workflow-fixture/scripts/prepare.mjs']) {
    assert.doesNotMatch(read(path), /(?:\/Users\/|\/Volumes\/|\/home\/[^/]+\/)/u);
  }
  assert.doesNotMatch(read('plugin/tests/workflow/page.test.cjs'), /exec(?:File)?\('agent-browser'/u);
});

test('UP-T004 UP-T055 plugin checks declare all desktop platforms and feature combinations', () => {
  const ci = read('.github/workflows/ci.yml');
  for (const os of ['ubuntu-latest', 'macos-latest', 'windows-latest']) assert.ok(ci.includes(os));
  for (const job of ['plugin-check:', 'fixture-build:', 'native-smoke:', 'page-runtime:', 'docs-and-skills:']) assert.ok(ci.includes(job));
  for (const features of ['xcap', 'native-screenshot', 'xcap,native-screenshot']) assert.ok(ci.includes(features));
  assert.ok(ci.includes('--no-default-features'));
  assert.doesNotMatch(ci, /continue-on-error:\s*true/u);
});

test('UP-T008 aggregate rejects failed, skipped, cancelled and absent required jobs', () => {
  const jobs = Object.fromEntries(['core-contracts', 'plugin-check', 'page-runtime', 'fixture-build', 'native-smoke', 'docs-and-skills', 'build'].map(name => [name, { result: 'success' }]));
  assert.doesNotThrow(() => assertRequiredJobs(jobs));
  for (const result of ['failure', 'skipped', 'cancelled', undefined]) {
    assert.throws(() => assertRequiredJobs({ ...jobs, 'plugin-check': { result } }), /plugin-check/u);
  }
  const { build, ...missing } = jobs;
  assert.throws(() => assertRequiredJobs(missing), /build/u);
});

test('UP-T081 native capture declares only its bounded diagnostic and ingress permissions', () => {
  const build = read('plugin/build.rs');
  const permission = read('plugin/permissions/default.toml');
  assert.match(build, /"capture_self_test"/);
  assert.match(build, /"push_capture_events"/);
  assert.match(permission, /"allow-capture-self-test"/);
  assert.match(permission, /"allow-push-capture-events"/);
  assert.doesNotMatch(permission, /allow-all|\*/);
});
