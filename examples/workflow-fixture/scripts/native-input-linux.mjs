// Real XTest input. Old distro xdotool --sync waits ~15s for a no-op move,
// so observe the cursor explicitly and keep preparation separate from input.
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {setTimeout as delay} from 'node:timers/promises';

export async function sendLinuxInput({action,x,y,exec=promisify(execFile),now=()=>performance.now(),sleep=delay}) {
  if(!['click','double','escape'].includes(action))throw new Error('Unsupported native input action');
  const deadline=now()+1500;
  const call=args=>{
    const remaining=Math.ceil(deadline-now());
    if(remaining<=0)throw new Error('Native pointer position preparation deadline exceeded');
    return exec('xdotool',args,{timeout:remaining,maxBuffer:65536});
  };
  if(action==='escape') {await call(['key','Escape']);return {action,pointerMoved:false};}
  const target={x:Math.round(x),y:Math.round(y)};
  if(!Number.isSafeInteger(target.x)||!Number.isSafeInteger(target.y))throw new Error('Invalid native target coordinates');
  const position=async()=>{
    const {stdout}=await call(['getmouselocation','--shell']);
    const values=Object.fromEntries(String(stdout).trim().split(/\r?\n/u).map(line=>line.split('=')));
    if(!/^-?\d+$/u.test(values.X||'')||!/^-?\d+$/u.test(values.Y||''))throw new Error('Invalid native pointer coordinates');
    const result={x:Number(values.X),y:Number(values.Y)};
    if(!Number.isSafeInteger(result.x)||!Number.isSafeInteger(result.y))throw new Error('Invalid native pointer coordinates');
    return result;
  };
  const same=point=>point.x===target.x&&point.y===target.y;
  let actual=await position();
  const alreadyAtTarget=same(actual);
  if(!alreadyAtTarget) {
    await call(['mousemove',String(target.x),String(target.y)]);
    while(true) {
      actual=await position();
      if(same(actual))break;
      const remaining=deadline-now();
      if(remaining<=0)throw new Error('Native pointer position preparation deadline exceeded');
      await sleep(Math.min(25,remaining));
    }
  }
  // No retry after this boundary: a failed process response may still have
  // posted its input. A double-click is one explicit two-event request.
  await call(action==='double'?['click','--repeat','2','--delay','100','1']:['click','1']);
  return {action,alreadyAtTarget,confirmedPosition:actual};
}
