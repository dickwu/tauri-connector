#!/usr/bin/env bun
/** Wait for element or text. Args: <selector_or_text> [--text] [timeout_ms] */
import { sendAndPrint } from './connector';

const arg = process.argv[2];
const isText = process.argv.includes('--text');
const timeoutArg = process.argv.find((a) => /^\d+$/.test(a) && a !== arg);
const timeout = timeoutArg ? parseInt(timeoutArg, 10) : 10000;

if (!arg) {
  console.error('Usage: bun run wait.ts <selector>         # wait for element');
  console.error('       bun run wait.ts "Success" --text   # wait for text');
  process.exit(1);
}

await sendAndPrint(
  {
    type: 'wait_for',
    ...(isText ? { text: arg } : { selector: arg, strategy: 'css' }),
    timeout,
    window_id: 'main',
  },
  timeout + 5000,
);
