#!/usr/bin/env bun
/** Take a screenshot. Optional args: [output_path] [max_width] */
import { send } from './connector';

const outPath = process.argv[2] || '/tmp/screenshot.png';
const maxWidth = process.argv[3] ? parseInt(process.argv[3], 10) : 1280;

const resp = await send(
  {
    type: 'screenshot',
    format: 'png',
    quality: 80,
    max_width: maxWidth,
    window_id: 'main',
  },
  60000,
) as { result?: { base64?: string; width?: number; height?: number }; error?: string };

if (resp.error) {
  console.error('Error:', resp.error);
  process.exit(1);
}

if (resp.result?.base64) {
  const buf = Buffer.from(resp.result.base64, 'base64');
  require('fs').writeFileSync(outPath, buf);
  console.log(`Saved ${outPath} (${buf.length} bytes, ${resp.result.width}x${resp.result.height})`);
} else {
  console.error('No screenshot data returned');
  process.exit(1);
}
