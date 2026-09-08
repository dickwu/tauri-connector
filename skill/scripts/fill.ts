#!/usr/bin/env bun
/** Fill one input: resolve the selector to exactly one element, replace its value, fire input events.
 * Args: <selector|@ref> <text> [--window <id>]
 * Uses the same targeted-input pipeline as CLI `fill` / MCP `webview_act_and_verify`: a selector that
 * matches zero or several elements fails (target_not_found / ambiguous_target) instead of typing into
 * whatever happens to have focus.
 */
import { send } from './connector';

const args = process.argv.slice(2);
const windowIndex = args.indexOf('--window');
const windowId = windowIndex >= 0 ? args[windowIndex + 1] ?? 'main' : 'main';
const positional =
  windowIndex >= 0 ? [...args.slice(0, windowIndex), ...args.slice(windowIndex + 2)] : args;
const [selector, text] = positional;

if (!selector || text === undefined) {
  console.error('Usage: bun run fill.ts <selector|@ref> <text> [--window <id>]');
  process.exit(1);
}

const response = await send({
  type: 'webview_act_and_verify',
  action: 'fill',
  selector,
  text,
  window_id: windowId,
});

if (response.error !== undefined) {
  console.error(JSON.stringify({ error: response.error, outcome: response.outcome }));
  process.exit(1);
}

const result = response.result as { verdict?: string; actionResult?: unknown } | undefined;
console.log(JSON.stringify(result?.actionResult ?? result ?? null, null, 2));
if (result?.verdict === 'failed') {
  console.error(JSON.stringify({ error: 'fill failed', outcome: response.outcome }));
  process.exit(1);
}
