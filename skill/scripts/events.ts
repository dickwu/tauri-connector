#!/usr/bin/env bun
/** Manage IPC event listeners. Subcommands: listen, captured, stop
 *
 *  listen <event1,event2,...>   Start listening for IPC events
 *  captured [--pattern regex] [--since ts] [--limit n]  Get captured events
 *  stop                        Stop listening for IPC events
 */
import { sendAndPrint } from './connector';

function getFlag(args: string[], long: string, short: string): string | undefined {
  const idx = args.findIndex(a => a === long || a === short);
  if (idx !== -1 && idx + 1 < args.length) {
    return args[idx + 1];
  }
  return undefined;
}

const argv = process.argv.slice(2);
const subcommand = argv[0];

if (!subcommand) {
  console.error('Usage: bun run events.ts <listen|captured|stop> [args]');
  process.exit(1);
}

if (subcommand === 'listen') {
  const eventArg = argv[1];
  if (!eventArg) {
    console.error('Usage: bun run events.ts listen <event1,event2,...>');
    process.exit(1);
  }

  const events = eventArg.split(',').map(e => e.trim()).filter(Boolean);
  await sendAndPrint({
    type: 'ipc_listen',
    action: 'start',
    events,
  });

} else if (subcommand === 'captured') {
  const pattern = getFlag(argv, '--pattern', '-p');
  const sinceRaw = getFlag(argv, '--since', '-s');
  const limitRaw = getFlag(argv, '--limit', '-n');

  const since = sinceRaw ? parseInt(sinceRaw, 10) : undefined;
  const limit = limitRaw ? parseInt(limitRaw, 10) : 100;

  await sendAndPrint({
    type: 'event_get_captured',
    ...(pattern ? { pattern } : {}),
    ...(since !== undefined ? { since } : {}),
    limit,
  });

} else if (subcommand === 'stop') {
  await sendAndPrint({
    type: 'ipc_listen',
    action: 'stop',
  });

} else {
  console.error(`Unknown subcommand: ${subcommand}`);
  console.error('Usage: bun run events.ts <listen|captured|stop> [args]');
  process.exit(1);
}
