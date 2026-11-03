// Real Chromium checks for plugin/js/scroll.js (gfit 0.17.7: `scroll down`
// moved only the document while the app scrolls an inner panel).
// Run: node plugin/tests/workflow/scroll-page.test.cjs
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { chromium } = require('playwright');

const helper = readFileSync(join(__dirname, '../../js/scroll.js'), 'utf8');
const tall = n => Array.from({ length: n }, (_, i) => `<div style="height:40px">row ${i}</div>`).join('');
const page = body => `<!doctype html><html><body style="margin:0">${body}</body></html>`;
// An app shell: the document never scrolls, the centred panel does.
const shell = page(`
  <div style="height:100vh;overflow:hidden;display:flex">
    <aside id="side" style="width:150px;overflow:auto">${tall(50)}</aside>
    <main id="panel" style="flex:1;overflow:auto">${tall(100)}<input id="field"></main>
  </div>`);

async function main() {
  const browser = await chromium.launch();
  const tab = await browser.newPage({ viewport: { width: 800, height: 600 } });
  const scroll = (selector, dx, dy) => tab.evaluate(([src, sel, x, y]) => {
    const fn = (0, eval)(src);
    return fn(sel ? document.querySelector(sel) : null, x, y);
  }, [helper, selector, dx, dy]);
  const results = [];
  const test = async (name, fn) => {
    try { await fn(); results.push({ name, passed: true }); }
    catch (e) { results.push({ name, passed: false, error: e.stack || String(e) }); }
  };
  const equal = (a, b, what) => {
    if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${what}: ${JSON.stringify(a)} != ${JSON.stringify(b)}`);
  };

  await test('no target scrolls the panel under the viewport centre, not the document', async () => {
    await tab.setContent(shell);
    const r = await scroll(null, 0, 200);
    equal(r.container.id, 'panel', 'container');
    equal(r.source, 'viewport-center', 'source');
    equal(r.scrolledBy, { x: 0, y: 200 }, 'scrolledBy');
    equal(await tab.evaluate(() => [window.scrollY, document.getElementById('panel').scrollTop]), [0, 200], 'positions');
  });
  await test('an explicit target scrolls its nearest scrollable ancestor', async () => {
    await tab.setContent(shell);
    const r = await scroll('#side div:nth-child(3)', 0, 120);
    equal(r.container.id, 'side', 'container');
    equal(r.source, 'selector', 'source');
    equal(r.scrolledBy.y, 120, 'scrolledBy');
  });
  await test('the focused element\'s container is used when the centre has none', async () => {
    await tab.setContent(page(`
      <div id="list" style="position:fixed;left:0;top:0;width:200px;height:200px;overflow:auto">${tall(30)}<input id="f"></div>
      <div style="height:100vh;overflow:hidden"></div>`));
    await tab.evaluate(() => { const l = document.getElementById('list'); document.getElementById('f').focus({ preventScroll: true }); l.scrollTop = 0; });
    const r = await scroll(null, 0, 100);
    equal(r.container.id, 'list', 'container');
    equal(r.source, 'focus', 'source');
    equal(r.scrolledBy.y, 100, 'scrolledBy');
  });
  await test('a panel at its edge gives way to the scrollable document', async () => {
    await tab.setContent(page(`
      <div style="height:300px"></div>
      <div id="short" style="height:200px;overflow:auto">${tall(6)}</div>
      ${tall(60)}`));
    await tab.evaluate(() => { const s = document.getElementById('short'); s.scrollTop = s.scrollHeight; window.scrollTo(0, 100); });
    const r = await scroll(null, 0, 150);
    equal(r.container.tag, 'document', 'container');
    equal(r.scrolledBy.y, 150, 'scrolledBy');
  });
  await test('plain documents still scroll, and smooth scrolling reports its final delta', async () => {
    await tab.setContent(page(`<style>html{scroll-behavior:smooth}</style>${tall(100)}`));
    const r = await scroll(null, 0, 300);
    equal(r.container.tag, 'document', 'container');
    equal(r.source, 'document', 'source');
    equal(r.scrolledBy.y, 300, 'scrolledBy');
    const up = await scroll(null, 0, -100);
    equal(up.scrolledBy.y, -100, 'up');
  });
  await test('nothing scrollable reports a zero delta instead of pretending', async () => {
    await tab.setContent(page('<p>short</p>'));
    const r = await scroll(null, 0, 300);
    equal(r.scrolledBy, { x: 0, y: 0 }, 'scrolledBy');
  });
  await test('horizontal scrolling picks a horizontally scrollable container', async () => {
    await tab.setContent(page(`<div style="height:100vh;overflow:hidden"><div id="wide" style="margin-top:250px;height:100px;overflow-x:auto;white-space:nowrap">${'<span style="display:inline-block;width:400px">x</span>'.repeat(10)}</div></div>`));
    const r = await scroll(null, 250, 0);
    equal(r.container.id, 'wide', 'container');
    equal(r.scrolledBy, { x: 250, y: 0 }, 'scrolledBy');
  });

  await browser.close();
  for (const r of results) console.log(`${r.passed ? 'ok' : 'not ok'} - ${r.name}${r.passed ? '' : `\n${r.error}`}`);
  if (results.some(r => !r.passed)) process.exit(1);
}
main().catch(e => { console.error(e); process.exit(1); });
