import { mkdirSync, writeFileSync, copyFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const require = createRequire(import.meta.url);
const root = fileURLToPath(new URL('../../../', import.meta.url));
const fixture = fileURLToPath(new URL('../', import.meta.url));
const reactDirectory = process.env.CONNECTOR_REACT_MODULE_DIR;
if (!reactDirectory) throw new Error('Set CONNECTOR_REACT_MODULE_DIR to an existing workspace containing React and react-dom. No dependencies are installed.');
const bundle = require(resolve(root, 'plugin/tests/workflow/react-fixture.cjs'));
mkdirSync(resolve(fixture, 'dist'), { recursive: true });
writeFileSync(resolve(fixture, 'dist/react.js'), bundle(resolve(reactDirectory)));
copyFileSync(resolve(root, 'plugin/tests/workflow/fixture-app.js'), resolve(fixture, 'dist/fixture-app.js'));
copyFileSync(resolve(fixture, 'frontend/index.html'), resolve(fixture, 'dist/index.html'));
console.log('Prepared isolated fixture assets from installed React.');
