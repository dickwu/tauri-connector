import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// An absent, cancelled, failed or skipped required job must fail the aggregate.
export function assertRequiredJobs(needs) {
  const expected = ['core-contracts', 'plugin-check', 'page-runtime', 'fixture-build', 'native-smoke', 'docs-and-skills', 'build'];
  const failures = expected.filter(name => needs[name]?.result !== 'success');
  if (failures.length) throw new Error(`Required CI jobs did not succeed: ${failures.join(', ')}`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  assertRequiredJobs(JSON.parse(process.env.CONNECTOR_CI_NEEDS || '{}'));
}
