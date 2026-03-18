#!/usr/bin/env bun
/** Focus an element and type text into it. Args: <selector> <text> */
import { send } from './connector';

const selector = process.argv[2];
const text = process.argv[3];
if (!selector || text === undefined) {
  console.error('Usage: bun run fill.ts <selector> <text>');
  process.exit(1);
}

// Focus, clear, then type
const script = `(() => {
  const el = document.querySelector("${selector.replace(/"/g, '\\"')}");
  if (!el) return { error: "Element not found: ${selector}" };
  el.focus();
  if ('value' in el) { el.value = ''; el.dispatchEvent(new Event('input', { bubbles: true })); }
  return { focused: true, tag: el.tagName.toLowerCase() };
})()`;

const focusResp = await send({ type: 'execute_js', script, window_id: 'main' });
if ((focusResp as { error?: string }).error) {
  console.error('Focus failed:', (focusResp as { error: string }).error);
  process.exit(1);
}

const typeResp = await send({
  type: 'keyboard',
  action: 'type',
  text,
  window_id: 'main',
});

if ((typeResp as { error?: string }).error) {
  console.error('Type failed:', (typeResp as { error: string }).error);
  process.exit(1);
}

console.log(JSON.stringify((typeResp as { result?: unknown }).result, null, 2));
