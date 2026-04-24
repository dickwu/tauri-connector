#!/usr/bin/env bun
/** DOM snapshot. Args: [mode] [selector]
 *  mode: ai (default) | accessibility | structure
 */
import { sendAndPrint } from './connector';

const mode = process.argv[2] || 'ai';
const selector = process.argv[3] || undefined;
const noReact = process.argv.includes('--no-react');
const noPortals = process.argv.includes('--no-portals');
const maxTokensIdx = process.argv.indexOf('--max-tokens');
const maxTokens = maxTokensIdx !== -1 ? parseInt(process.argv[maxTokensIdx + 1], 10) : undefined;
const noSplit = process.argv.includes('--no-split');

await sendAndPrint({
  type: 'dom_snapshot',
  mode,
  ...(selector && !selector.startsWith('--') ? { selector } : {}),
  ...(maxTokens !== undefined ? { max_tokens: maxTokens } : {}),
  ...(noSplit ? { no_split: true } : {}),
  react_enrich: !noReact,
  follow_portals: !noPortals,
  window_id: 'main',
});
