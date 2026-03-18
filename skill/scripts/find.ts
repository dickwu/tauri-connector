#!/usr/bin/env bun
/** Find elements by selector. Args: <selector> [strategy]
 *  strategy: css (default) | xpath | text
 */
import { sendAndPrint } from './connector';

const selector = process.argv[2];
if (!selector) {
  console.error('Usage: bun run find.ts <selector> [strategy]');
  process.exit(1);
}

await sendAndPrint({
  type: 'find_element',
  selector,
  strategy: process.argv[3] || 'css',
  window_id: 'main',
});
