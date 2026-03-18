#!/usr/bin/env bun
/** Read console logs. Args: [lines] [filter] */
import { send } from './connector';

const lines = parseInt(process.argv[2] || '20', 10);
const filter = process.argv[3] || undefined;

const resp = await send(
  {
    type: 'console_logs',
    lines,
    ...(filter ? { filter } : {}),
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
