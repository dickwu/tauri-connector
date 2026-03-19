#!/usr/bin/env bun
/** DOM snapshot. Args: [mode] [selector]
 *  mode: ai (default) | accessibility | structure
 */
import { sendAndPrint } from './connector';

const mode = process.argv[2] || 'ai';
const selector = process.argv[3] || undefined;
const noReact = process.argv.includes('--no-react');
const noPortals = process.argv.includes('--no-portals');

await sendAndPrint({
  type: 'dom_snapshot',
  mode,
  ...(selector && !selector.startsWith('--') ? { selector } : {}),
  react_enrich: !noReact,
  follow_portals: !noPortals,
  window_id: 'main',
});
