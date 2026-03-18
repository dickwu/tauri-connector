#!/usr/bin/env bun
/** Take a DOM snapshot. Args: [type] [selector]
 *  type: accessibility (default) | structure
 */
import { sendAndPrint } from './connector';

const snapshotType = process.argv[2] || 'accessibility';
const selector = process.argv[3] || undefined;

await sendAndPrint({
  type: 'dom_snapshot',
  snapshot_type: snapshotType,
  ...(selector ? { selector } : {}),
  window_id: 'main',
});
