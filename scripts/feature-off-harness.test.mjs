import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {test} from 'node:test';

const root=fileURLToPath(new URL('../',import.meta.url));
const harness=resolve(root,'examples/workflow-fixture/scripts/feature-off-test.mjs');

// The surrogate is a plain Node process with no listener or native WebView.
// Preloading only affects the argv-less child launched as CONNECTOR_FIXTURE_BINARY.
async function execute(mode) {
  const directory=mkdtempSync(resolve(tmpdir(),'connector-feature-off-harness-'));
  const loader=resolve(directory,'surrogate.mjs');
  const output=resolve(directory,'evidence');
  writeFileSync(loader, `
    import {writeFileSync} from 'node:fs';
    if (process.argv.length === 1) {
      setInterval(() => {}, 1000);
      if (process.env.CONNECTOR_FEATURE_OFF_TEST_MODE === 'signal') setTimeout(() => process.kill(process.pid, 'SIGTERM'), 20);
      if (process.env.CONNECTOR_FEATURE_OFF_TEST_MODE === 'normal_exit') setTimeout(() => process.exit(0), 20);
      if (process.env.CONNECTOR_FEATURE_OFF_TEST_MODE === 'boot_contract') writeFileSync(process.env.CONNECTOR_FIXTURE_BOOT_EVIDENCE,JSON.stringify({processId:process.pid,windowId:'main',connectorFeature:false,documentReadyState:'complete',fixtureReady:true,connectorGlobals:[],hookMarkers:[]}));
    }
  `);
  const child=spawn(process.execPath,[harness],{
    cwd:root,
    env:{...process.env,CONNECTOR_FIXTURE_BINARY:process.execPath,CONNECTOR_FIXTURE_OUTPUT:output,
      CONNECTOR_FEATURE_OFF_TEST_MODE:mode,NODE_OPTIONS:`${process.env.NODE_OPTIONS||''} --import=${pathToFileURL(loader).href}`},
    stdio:['ignore','pipe','pipe'],
  });
  let stdout='',stderr='';
  child.stdout.on('data',data=>stdout+=data);child.stderr.on('data',data=>stderr+=data);
  try {
    const ended=await new Promise((resolveExit,reject)=>{
      const timeout=setTimeout(()=>{child.kill('SIGKILL');reject(Error('surrogate harness exceeded its deadline'));},15000);
      child.once('error',error=>{clearTimeout(timeout);reject(error);});
      child.once('close',(code,signal)=>{clearTimeout(timeout);resolveExit({code,signal});});
    });
    const reportPath=resolve(output,'result.json');
    return {...ended,stdout,stderr,report:existsSync(reportPath)?JSON.parse(readFileSync(reportPath,'utf8')):null};
  } finally {
    if(child.exitCode===null&&child.signalCode===null)child.kill('SIGKILL');
    rmSync(directory,{recursive:true,force:true});
  }
}

test('UP-T005 signalled surrogate must not produce a native-running success claim',async()=>{
  const result=await execute('signal');
  assert.notEqual(result.code,0,'A signalled child currently passes the feature-off harness');
  assert.equal(result.report?.passed===true,false);
  assert.match(result.stderr,/must remain running|signal|no longer alive/i);
});

test('UP-T005 early clean child exit is also rejected',async()=>{
  const result=await execute('normal_exit');
  assert.notEqual(result.code,0);assert.equal(result.report?.passed===true,false);
  assert.match(result.stderr,/must remain running|no longer alive/i);
});

test('UP-T005 live surrogate without frontend boot cannot claim native readiness',async()=>{
  const result=await execute('alive');
  assert.notEqual(result.code,0);assert.equal(result.report?.passed===true,false);
  assert.match(result.stderr,/frontend boot evidence missing/);
});

test('UP-T005 synthetic boot record validates harness contract only, never native evidence',async()=>{
  const result=await execute('boot_contract');
  assert.equal(result.code,0,result.stderr);assert.equal(result.signal,null);
  assert.equal(result.report.passed,true);assert.equal(result.report.nativeAppRunning,true);
  assert.equal(result.report.frontendBoot.fixtureReady,true);assert.deepEqual(result.report.frontendBoot.connectorGlobals,[]);
  assert.deepEqual(result.report.checkedPorts,[19555,19556]);
  // This temporary surrogate report is discarded and never used as native evidence.
});
