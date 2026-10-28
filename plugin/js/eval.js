// Runs a caller's `execute_js` script with devtools-like semantics and a
// page-side deadline.
// 1. An expression keeps its historical meaning: `return (<script>)`.
// 2. Otherwise a statement list that compiles as script code (no top-level
//    `return`/`await`) runs through a block-scoped indirect eval so its value
//    is the last statement's completion value, e.g. `const a = 1; a + 1` is 2.
//    The syntax check compiles without running, so nothing executes twice.
// 3. Otherwise the script is an async function body (`await`, `return`).
//    With no `return` in it, its last top-level statement is returned when
//    it is an expression (`await load(); "x"` is "x"), like devtools.
// `source` may instead be a function the plugin compiled natively (a CSP
// without 'unsafe-eval' blocks the string forms); it is run as is.
// A promise still pending at the deadline rejects with `timeoutMark` so the
// bridge reports a clear timeout instead of a transport timeout with an
// unknown outcome, but only while the effect monitor proves no IPC/network
// effect started. Once an effect may have started (or no monitor is present)
// the promise is left pending: the bridge deadline then reports
// outcome_unknown and a late result can still settle the quarantine.
(function connectorEval(source, deadlineMs, timeoutMark) {
  'use strict';
  const fx = typeof window !== 'undefined' ? window.__CONNECTOR_EFFECTS__ : undefined;
  const mark = fx ? fx.mark() : 0;
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  let run;
  if (typeof source === 'function') {
    run = source;
  } else try {
    const expression = new AsyncFunction('return (' + source + '\n)');
    run = () => expression();
  } catch (expressionError) {
    if (!(expressionError instanceof SyntaxError)) throw expressionError;
    let statements = false;
    try {
      // Class static blocks reject top-level return/await like script code.
      new Function('(class{static{\n' + source + '\n}})');
      statements = true;
    } catch (_) {}
    if (statements) {
      run = () => (0, eval)('{\n' + source + '\n}');
    } else {
      const body = new AsyncFunction(source);
      const last = returningLastExpression(source, AsyncFunction);
      run = last ? () => last() : () => body();
    }
  }
  // Compiles `head; return (tail)` for the last top-level statement boundary
  // (a `;`, or a newline ASI would end a statement at) whose tail is an
  // expression; null when the body has a `return` or no such split parses.
  // Strings, templates and comments are skipped; nothing runs here.
  function returningLastExpression(src, AsyncFn) {
    const cuts = [];
    const stack = [];
    let i = 0;
    while (i < src.length) {
      const c = src[i];
      const n = src[i + 1];
      if (c === '/' && n === '/') { const e = src.indexOf('\n', i); i = e < 0 ? src.length : e; continue; }
      if (c === '/' && n === '*') { const e = src.indexOf('*/', i + 2); i = e < 0 ? src.length : e + 2; continue; }
      if (c === '"' || c === "'") {
        i++;
        while (i < src.length && src[i] !== c && src[i] !== '\n') i += src[i] === '\\' ? 2 : 1;
        i++;
        continue;
      }
      if (c === '`') { stack.push('`'); i++; continue; }
      if (stack[stack.length - 1] === '`') {
        while (i < src.length && src[i] !== '`' && !(src[i] === '$' && src[i + 1] === '{')) i += src[i] === '\\' ? 2 : 1;
        if (src[i] === '`') { stack.pop(); i++; } else if (i < src.length) { stack.push('${'); i += 2; }
        continue;
      }
      if (c === '(' || c === '[' || c === '{') stack.push(c);
      else if (c === ')' || c === ']') stack.pop();
      else if (c === '}') { if (stack.pop() === '${') { i++; continue; } }
      if (stack.length === 0) {
        if (c === ';' || c === '\n' || c === '}') cuts.push(i + 1);
        if (/[A-Za-z0-9_$]/.test(c) && src.startsWith('return', i) && !/[A-Za-z0-9_$]/.test(src[i - 1] || '') && !/[A-Za-z0-9_$]/.test(src[i + 6] || '')) return null;
      }
      i++;
    }
    for (let k = cuts.length - 1; k >= 0; k--) {
      const tail = src.slice(cuts[k]).trim().replace(/;+\s*$/, '').trim();
      // Only a `;` surely ends a statement; after a newline or `}` a tail
      // that could continue the expression is not split off.
      if (!tail || (src[cuts[k] - 1] !== ';' && /^[([`+\-*/%.,?:=<>&|!]/.test(tail))) continue;
      if (/^(in|instanceof|of)\b/.test(tail)) continue;
      try {
        return new AsyncFn(src.slice(0, cuts[k]) + '\nreturn (' + tail + '\n);');
      } catch (_) {}
    }
    return null;
  }
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => {
      if (fx && fx.unchangedSince(mark)) {
        reject(new Error(timeoutMark + ': script still pending after ' + deadlineMs + ' ms'));
      }
    }, deadlineMs);
  });
  return Promise.race([Promise.resolve().then(run), deadline]).finally(() => clearTimeout(timer));
})
