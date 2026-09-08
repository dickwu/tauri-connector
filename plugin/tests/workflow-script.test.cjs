const test = require('node:test');
const assert = require('node:assert/strict');
const script = import('../../skill/scripts/workflow.ts');

test('workflow helper keeps host credentials outside the spec', async () => {
  const { prepareRequest } = await script;
  const spec = { schemaVersion: 1, runKey: 'fixture', steps: [] };
  const result = prepareRequest('run', { spec }, 'x'.repeat(32));
  assert.equal(result.operation, 'workflow_run');
  assert.equal(result.args.authToken, 'x'.repeat(32));
  assert.deepEqual(result.args.spec, spec);
  assert.equal(spec.authToken, undefined);
});

test('workflow helper rejects unknown operations and missing credentials', async () => {
  const { prepareRequest } = await script;
  assert.throws(() => prepareRequest('execute_js', {}, 'x'.repeat(32)));
  assert.throws(() => prepareRequest('run', {}, undefined));
  assert.deepEqual(prepareRequest('capabilities', {}, undefined).args, {});
});

test('workflow helper preserves failed and incomplete exit statuses', async () => {
  const { reportExitCode } = await script;
  assert.equal(reportExitCode({status:'completed',goalStatus:'not_requested'}), 0);
  assert.equal(reportExitCode({status:'completed',originalTestVerdict:'failed'}), 1);
  assert.equal(reportExitCode({status:'paused'}), 2);
  assert.equal(reportExitCode({status:'cancelled'}), 1);
});
