#!/usr/bin/env bun
/** Execute JavaScript in the webview. Pass script as first arg, optional --timeout <ms>. */
import { sendAndPrint } from './connector';

const args = process.argv.slice(2);
const timeoutAt = args.indexOf('--timeout');
const timeoutMs = timeoutAt >= 0 ? Number(args.splice(timeoutAt, 2)[1]) : undefined;
const script = args[0];
if (!script || !script.trim() || (timeoutMs !== undefined && !(timeoutMs > 0))) {
  console.error('Usage: bun run eval.ts <js-expression> [--timeout <ms>]');
  console.error('  e.g. bun run eval.ts "document.title"');
  process.exit(1);
}

await sendAndPrint(
  {
    type: 'execute_js',
    script,
    window_id: 'main',
    ...(timeoutMs !== undefined ? { timeout_ms: timeoutMs } : {}),
  },
  (timeoutMs ?? 30_000) + 5_000,
);
