#!/usr/bin/env bun
/** Hover on/off an element. Args: <selector> [strategy] [--off]
 *  strategy: css (default) | xpath | text
 *  --off: fire leave events to dismiss dropdown/tooltip
 */
import { sendAndPrint } from './connector';

const args = process.argv.slice(2);
const off = args.includes('--off');
const filtered = args.filter((a) => a !== '--off');
const selector = filtered[0];

if (!selector) {
  console.error('Usage: bun run hover.ts <selector> [strategy] [--off]');
  process.exit(1);
}

await sendAndPrint({
  type: 'interact',
  action: off ? 'hover-off' : 'hover',
  selector,
  strategy: filtered[1] || 'css',
  window_id: 'main',
});
