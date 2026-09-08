// Test-only bundling of an already installed React development build. The
// module directory is explicit; this never installs or changes dependencies.
const { readFileSync } = require('node:fs');
const { dirname } = require('node:path');

module.exports = function bundleReact(moduleDirectory) {
  const modules = new Map();
  function add(filename) {
    if (modules.has(filename)) return;
    const source = readFileSync(filename, 'utf8');
    const dependencies = {};
    modules.set(filename, { source, dependencies });
    for (const match of source.matchAll(/\brequire\(['"]([^'"]+)['"]\)/gu)) {
      const resolved = require.resolve(match[1], { paths: [dirname(filename)] });
      dependencies[match[1]] = resolved;
      add(resolved);
    }
  }
  const react = require.resolve('react', { paths: [moduleDirectory] });
  const client = require.resolve('react-dom/client', { paths: [moduleDirectory] });
  add(react); add(client);
  const wrappers = Array.from(modules, ([filename, module]) => `${JSON.stringify(filename)}:[function(module,exports,require){\n${module.source}\n},${JSON.stringify(module.dependencies)}]`);
  return `(() => {const process={env:{NODE_ENV:'development'}};const modules={${wrappers.join(',')}};const cache={};function load(id){if(cache[id])return cache[id].exports;const module={exports:{}};cache[id]=module;const [fn,deps]=modules[id];fn(module,module.exports,name=>load(deps[name]));return module.exports;}window.WorkflowReact={React:load(${JSON.stringify(react)}),ReactDOM:load(${JSON.stringify(client)})};})()`;
};
