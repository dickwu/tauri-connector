#!/usr/bin/env bun
/**
 * List or release resource quarantines.
 *   bun run resources.ts                      # list
 *   bun run resources.ts release <requestId>  # release one after checking the app
 *   bun run resources.ts release --all
 */
import { sendAndPrint } from './connector';

const [action = 'list', target] = process.argv.slice(2);
if (action === 'list') {
  await sendAndPrint({ type: 'resources', action: 'list' });
} else if (action === 'release' && target) {
  await sendAndPrint(
    target === '--all'
      ? { type: 'resources', action: 'release', all: true }
      : { type: 'resources', action: 'release', request_id: target },
  );
} else {
  console.error('Usage: bun run resources.ts [list | release <requestId> | release --all]');
  process.exit(1);
}
