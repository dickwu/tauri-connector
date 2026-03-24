#!/usr/bin/env bun
/** Drag an element to a target.
 *  Args: <source-selector> <target-selector-or-x,y> [--steps N] [--duration N] [--strategy auto|pointer|html5dnd]
 */
import { send } from './connector';

const args = process.argv.slice(2);
const flags: Record<string, string> = {};
const positional: string[] = [];

for (let i = 0; i < args.length; i++) {
  if (args[i] === '--steps' && args[i + 1]) {
    flags.steps = args[++i];
  } else if (args[i] === '--duration' && args[i + 1]) {
    flags.duration = args[++i];
  } else if (args[i] === '--strategy' && args[i + 1]) {
    flags.strategy = args[++i];
  } else {
    positional.push(args[i]);
  }
}

const source = positional[0];
const target = positional[1];

if (!source || !target) {
  console.error('Usage: bun run drag.ts <source-selector> <target-selector|x,y> [--steps 10] [--duration 300] [--strategy auto]');
  process.exit(1);
}

const steps = parseInt(flags.steps || '10', 10);
const durationMs = parseInt(flags.duration || '300', 10);
const strategy = flags.strategy || 'auto';

// Build the drag request
const msg: Record<string, unknown> = {
  type: 'interact',
  action: 'drag',
  selector: source,
  strategy: 'css',
  steps,
  duration_ms: durationMs,
  drag_strategy: strategy,
  window_id: 'main',
};

// Resolve target: "x,y" coordinates or CSS selector
const coordMatch = target.match(/^(\d+(?:\.\d+)?)\s*,\s*(\d+(?:\.\d+)?)$/);
if (coordMatch) {
  msg.target_x = parseFloat(coordMatch[1]);
  msg.target_y = parseFloat(coordMatch[2]);
} else {
  msg.target_selector = target;
}

const resp = await send(msg, durationMs + 10_000);
if ((resp as { error?: string }).error) {
  console.error('Error:', (resp as { error: string }).error);
  process.exit(1);
}
console.log(JSON.stringify((resp as { result?: unknown }).result, null, 2));
