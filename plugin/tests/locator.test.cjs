const assert = require('node:assert/strict');
const {test} = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const crypto = require('node:crypto').webcrypto;
function node(id, attrs, text) {
  return { id, tagName: 'BUTTON', nodeType: 1, textContent: text, children: [], parentElement: null,
    getAttribute(name) { return attrs[name] || null; }, hasAttribute(name) { return name in attrs; },
    getBoundingClientRect() { return {x:0,y:0,width:10,height:10}; }, getClientRects() { return [1]; }, closest() { return null; }, click() { this.clickCount = (this.clickCount || 0) + 1; }};
}
async function locate(query, nodes) {
  const document = { querySelectorAll(selector) { return selector === '*' ? nodes : []; }, getElementById() { return null; } };
  const context = {document, crypto, CSS:{escape: x=>x}, getComputedStyle:()=>({display:'block',visibility:'visible',opacity:'1'})};
  context.window = context;
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/semantic/core.js'), 'utf8'), context);
  const helper = path.join(__dirname, '../js/locator.js');
  return vm.runInNewContext(fs.readFileSync(helper, 'utf8'), context)(query, null);
}
test('B09 role and text constraints intersect', async () => {
  const save = node('save', {}, 'Save'); const cancel = node('cancel', {}, 'Cancel');
  const result = await locate({role:'button', text:'Save', exact:true}, [save,cancel]);
  assert.equal(result.count, 1); assert.equal(result.matched.selector, '#save');
});
test('B09 all provided attributes intersect rather than union', async () => {
  const a = node('a', {'data-testid':'desired', title:'Wrong'}, 'Save');
  const b = node('b', {'data-testid':'other', title:'Desired'}, 'Save');
  const result = await locate({testId:'desired', title:'Desired', exact:true}, [a,b]);
  assert.equal(result.count, 0); assert.equal(a.clickCount || 0, 0);
});
test('B09 text in a child does not discard the parent matching an explicit role', async () => {
  const save = node('save', {}, 'Save');
  const child = node('caption', {}, 'Save'); child.tagName = 'SPAN'; save.children = [child];
  const result = await locate({role:'button', text:'Save', exact:true}, [save, child]);
  assert.equal(result.count, 1); assert.equal(result.matched.selector, '#save');
});
test('git-strata --name alone finds the element by accessible name', async () => {
  const input = node('filter', {'aria-label': 'Filter sessions'}, ''); input.tagName = 'INPUT';
  const other = node('save', {}, 'Save');
  const result = await locate({name: 'Filter sessions'}, [input, other]);
  assert.equal(result.count, 1); assert.equal(result.matched.selector, '#filter');
});
test('git-strata a --name miss lists the names the other filters found', async () => {
  const input = node('filter', {'aria-label': 'Search sessions'}, ''); input.tagName = 'INPUT';
  const result = await locate({role: 'textbox', name: 'Filter sessions'}, [input]);
  assert.equal(result.code, 'target_not_found');
  assert.equal(result.candidates.length, 1); assert.equal(result.candidates[0].name, 'Search sessions');
  assert.match(result.error, /did not match/);
});
