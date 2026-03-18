#!/usr/bin/env bun
/** Execute JavaScript in the webview. Pass script as first arg. */
import { sendAndPrint } from './connector';

const script = process.argv[2];
if (!script) {
  console.error('Usage: bun run eval.ts <js-expression>');
  console.error('  e.g. bun run eval.ts "document.title"');
  process.exit(1);
}

await sendAndPrint({
  type: 'execute_js',
  script,
  window_id: 'main',
});
