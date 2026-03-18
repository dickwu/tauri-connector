#!/usr/bin/env bun
/** Get app metadata (name, version, environment, windows). No bridge needed. */
import { sendAndPrint } from './connector';
await sendAndPrint({ type: 'backend_state' }, 5000);
