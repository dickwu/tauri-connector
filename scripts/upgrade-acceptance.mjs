import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const data = JSON.parse(readFileSync(resolve(root, 'docs/upgrade-acceptance.json'), 'utf8'));
const dimensions = ['implemented', 'unitVerified', 'integrationVerified', 'nativeVerified', 'ciVerified'];
const ids = [...Array.from({ length: 108 }, (_, i) => `UP-T${String(i + 1).padStart(3, '0')}`),
  ...Array.from({ length: 36 }, (_, i) => `UP-PK${String(i + 1).padStart(3, '0')}`)];
assert.deepEqual(data.acceptance.map(row => row.id), ids, 'Every acceptance ID must occur exactly once and in spec order');
for (const row of data.acceptance) {
  for (const field of dimensions) {
    assert.ok(['pending', 'partial', 'complete', 'passed', 'failed', 'blocked', 'not_applicable'].includes(row[field]?.status), `${row.id}.${field} needs an explicit state`);
    assert.ok(Array.isArray(row[field].evidence), `${row.id}.${field} needs evidence links`);
    if (['complete', 'passed'].includes(row[field].status)) assert.ok(row[field].evidence.length, `${row.id}.${field} cannot pass without evidence`);
    for (const path of row[field].evidence) assert.ok(existsSync(resolve(root, path.split('#')[0])), `${row.id}: evidence ${path} does not exist`);
  }
  for (const platform of ['macos', 'windows', 'linux']) assert.ok(row.nativePlatforms[platform], `${row.id} needs ${platform} native state`);
}

const escape = value => String(value).replaceAll('|', '\\|').replaceAll('\n', ' ');
const cell = state => escape(state.status);
let report = `# Upgrade v1.1 implementation and verification status\n\n`;
report += `Current delivery state: **${data.overallStatus}**. The 144 requirements below are tracked individually; pending requirements remain in scope.\n\n`;
report += `Baseline HEAD: \`${data.baseline.head}\`; initial worktree: ${data.baseline.worktree}. `;
report += `Fresh tool versions, commands, test names and log hashes: [baseline evidence](upgrade-evidence/baseline.json). `;
report += `The machine-readable source is [upgrade-acceptance.json](upgrade-acceptance.json); regenerate this page with \`npm run acceptance:render\`.\n\n${data.statusPolicy}\n\n`;
report += `## Baseline audit\n\n| ID | State | Observed baseline |\n| --- | --- | --- |\n`;
for (const row of data.baselineFindings) report += `| ${row.id} | ${row.status} | ${escape(row.note)} |\n`;
report += `\n## Implementation requirements\n\n| Requirement | State | Evidence |\n| --- | --- | --- |\n`;
for (const row of data.requirements) report += `| ${row.id} ${escape(row.title)} | ${row.status} | ${row.evidence.map(path => `[${path}](../${path})`).join(', ')} |\n`;
report += `\n## Platform capability and evidence\n\n| Platform | Code | Build | Native | CI | Boundary |\n| --- | --- | --- | --- | --- | --- |\n`;
for (const [name, row] of Object.entries(data.platforms)) report += `| ${name} | ${row.code} | ${row.build} | ${row.native} | ${row.ci} | ${escape(row.note)} |\n`;
report += `\n## Acceptance matrix\n\nEach row retains its required assertion and verification layer in the JSON. Evidence may support several requirements, but each state is independently assigned. \`nativeVerified\` never follows from a service mock or Chromium result.\n\n`;
report += `| ID / scenario | implemented | unitVerified | integrationVerified | nativeVerified | ciVerified | Evidence / boundary |\n| --- | --- | --- | --- | --- | --- | --- |\n`;
for (const row of data.acceptance) {
  const evidence = [...new Set(dimensions.flatMap(field => row[field].evidence))];
  const notes = dimensions.filter(field => !['pending', 'not_applicable'].includes(row[field].status)).map(field => row[field].note).filter(Boolean);
  const details = evidence.map(path => `[${path}](../${path})`).join(', ') + (notes.length ? ` — ${escape([...new Set(notes)].join(' '))}` : `Required: ${escape(row.requiredLayer)}; evidence pending.`);
  report += `| ${row.id} ${escape(row.scenario)} | ${dimensions.map(field => cell(row[field])).join(' | ')} | ${details} |\n`;
}
report += `\n## Known limits and remaining verification\n\n${data.knownLimits.map(note => `- ${note}`).join('\n')}\n\n## Reproduction commands\n\nRun from the repository root with the checked-in lockfiles. Native smoke requires the isolated fixture and a desktop session (Linux CI uses Xvfb/WebKitGTK).\n\n\`\`\`sh\n${data.reproduction.join('\n')}\n\`\`\`\n`;
const target = resolve(root, 'docs/upgrade-implementation-status.md');
if (process.argv.includes('--check')) {
  assert.equal(readFileSync(target, 'utf8'), report, 'Acceptance Markdown is stale; run npm run acceptance:render');
  console.log('PASS: 144 distinct IDs, independent evidence states, existing evidence links, synchronized report');
} else {
  writeFileSync(target, report);
  console.log('Rendered 144 acceptance rows and implementation/platform status');
}
