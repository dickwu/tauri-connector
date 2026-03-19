#!/usr/bin/env bun
/** Read console logs. Args: [lines] [filter] [--level <level>] [--pattern <regex>]
 *  --level / -l   Filter by log level (log, warn, error, info, debug)
 *  --pattern / -p Filter messages by regex pattern
 */
import { send } from './connector';

// Parse --level / -l and --pattern / -p from CLI args
function getFlag(args: string[], long: string, short: string): string | undefined {
  const idx = args.findIndex(a => a === long || a === short);
  if (idx !== -1 && idx + 1 < args.length) {
    return args[idx + 1];
  }
  return undefined;
}

const argv = process.argv.slice(2);
const level = getFlag(argv, '--level', '-l');
const pattern = getFlag(argv, '--pattern', '-p');

// Positional args: skip any consumed flag pairs
const positional = argv.filter((a, i) => {
  if (a === '--level' || a === '-l' || a === '--pattern' || a === '-p') return false;
  const prev = i > 0 ? argv[i - 1] : '';
  if (prev === '--level' || prev === '-l' || prev === '--pattern' || prev === '-p') return false;
  return true;
});

const lines = parseInt(positional[0] || '20', 10);
const filter = positional[1] || undefined;

const resp = await send(
  {
    type: 'console_logs',
    lines,
    ...(filter ? { filter } : {}),
    ...(level ? { level } : {}),
    ...(pattern ? { pattern } : {}),
    window_id: 'main',
  },
  5000,
) as { result?: { logs?: Array<{ level: string; message: string }> }; error?: string };

if (resp.error) {
  console.error('Error:', resp.error);
  process.exit(1);
}

const logs = resp.result?.logs || [];
for (const l of logs) {
  console.log(`[${l.level}] ${l.message}`);
}
console.log(`--- ${logs.length} logs ---`);
