#!/usr/bin/env bun
/** Click an element. Args: <selector> [strategy]
 *  strategy: css (default) | xpath | text
 */
import { sendAndPrint } from './connector';

const selector = process.argv[2];
if (!selector) {
  console.error('Usage: bun run click.ts <selector> [strategy]');
  process.exit(1);
}

await sendAndPrint({
  type: 'interact',
  action: 'click',
  selector,
  strategy: process.argv[3] || 'css',
  window_id: 'main',
});
