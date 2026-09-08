const assert=require('node:assert/strict');
const {test}=require('node:test');
const fs=require('node:fs'); const path=require('node:path'); const vm=require('node:vm');
function monitor(window,expected,on){
  const file=path.join(__dirname,'../js/ipc-monitor.js');
  return vm.runInNewContext(fs.readFileSync(file,'utf8'),{window})(expected,on,'page-epoch');
}
test('T14 monitor refuses wrong window before altering flags',()=>{
 const window={__TAURI_INTERNALS__:{metadata:{currentWindow:{label:'other'}}}};
 const result=monitor(window,'main',true);
 assert.equal(result.code,'observation_failed'); assert.equal(window.__CONNECTOR_IPC_MONITOR__,undefined);
});
test('T14 monitor acknowledges actual window, page epoch and applied state',()=>{
 const window={__TAURI_INTERNALS__:{metadata:{currentWindow:{label:'main'}}}};
 const result=monitor(window,'main',true);
 assert.equal(result.windowId,'main');assert.equal(result.applied,true); assert.equal(result.pageEpoch,'page-epoch');
});
