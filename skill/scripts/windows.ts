#!/usr/bin/env bun
/** List all windows or get info for a specific window. Args: [window_id] */
import { sendAndPrint } from './connector';

const windowId = process.argv[2];

if (windowId) {
  await sendAndPrint({ type: 'window_info', window_id: windowId }, 5000);
} else {
  await sendAndPrint({ type: 'window_list' }, 5000);
}
