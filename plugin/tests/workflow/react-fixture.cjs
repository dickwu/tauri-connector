// Test-only bundling of locally installed React. npm ci provides the default
// version from the repository lockfile; no helper is loaded from a CDN.
const { readFileSync } = require('node:fs');
const { dirname, relative, sep } = require('node:path');

module.exports = function bundleReact(moduleDirectory) {
  const moduleId = filename => relative(moduleDirectory, filename).split(sep).join('/');
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
  const wrappers = Array.from(modules, ([filename, module]) => `${JSON.stringify(moduleId(filename))}:[function(module,exports,require){\n${module.source}\n},${JSON.stringify(Object.fromEntries(Object.entries(module.dependencies).map(([name, path]) => [name, moduleId(path)])))}]`);
  return `(() => {const process={env:{NODE_ENV:'development'}};const modules={${wrappers.join(',')}};const cache={};function load(id){if(cache[id])return cache[id].exports;const module={exports:{}};cache[id]=module;const [fn,deps]=modules[id];fn(module,module.exports,name=>load(deps[name]));return module.exports;}window.WorkflowReact={React:load(${JSON.stringify(moduleId(react))}),ReactDOM:load(${JSON.stringify(moduleId(client))})};})()`;
};
