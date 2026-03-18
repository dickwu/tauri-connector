#!/usr/bin/env bun
/** Take a screenshot. Optional args: [output_path] [max_width]
 *  Format is inferred from extension: .png (default), .jpg/.jpeg, .webp
 */
import { send } from './connector';

const outPath = process.argv[2] || '/tmp/screenshot.png';
const maxWidth = process.argv[3] ? parseInt(process.argv[3], 10) : 1280;

const ext = outPath.split('.').pop()?.toLowerCase() || 'png';
const format = ext === 'jpg' || ext === 'jpeg' ? 'jpeg' : ext === 'webp' ? 'webp' : 'png';

const resp = await send(
  {
    type: 'screenshot',
    format,
    quality: 80,
    max_width: maxWidth,
    window_id: 'main',
  },
  60000,
) as { result?: { base64?: string; width?: number; height?: number; method?: string }; error?: string };

if (resp.error) {
  console.error('Error:', resp.error);
  process.exit(1);
}

if (resp.result?.base64) {
  const buf = Buffer.from(resp.result.base64, 'base64');
  require('fs').writeFileSync(outPath, buf);
  const method = resp.result.method || 'unknown';
  console.log(`Saved ${outPath} (${buf.length} bytes, ${resp.result.width}x${resp.result.height}, ${method})`);
} else {
  console.error('No screenshot data returned');
  process.exit(1);
}
