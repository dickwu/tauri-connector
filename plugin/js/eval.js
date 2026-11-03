// Runs a caller's `execute_js` script with devtools-like semantics and a
// page-side deadline.
// 1. An expression keeps its historical meaning: `return (<script>)`.
// 2. Otherwise a statement list that compiles as script code (no top-level
//    `return`/`await`) runs through a block-scoped indirect eval so its value
//    is the last statement's completion value, e.g. `const a = 1; a + 1` is 2.
//    The syntax check compiles without running, so nothing executes twice.
// 3. Otherwise the script is an async function body (`await`, `return`).
//    With no `return` in it, its last top-level statement is returned when
//    it is an expression (`await load(); "x"` is "x"), like devtools, and a
//    trailing block, try/catch/finally or if/else yields the value of its
//    last statement the way sync code would.
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
      const last = returningLastStatement(source, AsyncFunction);
      run = last ? () => last() : () => body();
    }
  }
  // Compiles the body with its last top-level statement turned into a
  // `return`, so an async body keeps sync completion-value semantics: an
  // expression becomes `return (expr)`, and a trailing block, `try`/`catch`
  // (`finally` keeps its own value out, like sync code) or `if`/`else` gets
  // the same rewrite on the last statement of each branch (gfit 0.17.7:
  // `try { await x; "v" } catch (e) {}` returned null). Null when the body
  // has a top-level `return` or nothing parses. Strings, templates and
  // comments are skipped; nothing runs here.
  function returningLastStatement(src, AsyncFn) {
    const compiles = (code) => { try { new AsyncFn(code); return true; } catch (_) { return false; } };
    const ident = (ch) => /[A-Za-z0-9_$]/.test(ch || '');
    // Index just past the comment, string or template starting at i, or -1.
    function skipToken(text, i) {
      const c = text[i];
      const n = text[i + 1];
      if (c === '/' && n === '/') { const e = text.indexOf('\n', i); return e < 0 ? text.length : e; }
      if (c === '/' && n === '*') { const e = text.indexOf('*/', i + 2); return e < 0 ? text.length : e + 2; }
      if (c === '"' || c === "'") {
        i++;
        while (i < text.length && text[i] !== c && text[i] !== '\n') i += text[i] === '\\' ? 2 : 1;
        return i + 1;
      }
      if (c === '`') {
        i++;
        while (i < text.length && text[i] !== '`') {
          if (text[i] === '\\') { i += 2; continue; }
          if (text[i] === '$' && text[i + 1] === '{') {
            const e = closeOf(text, i + 1);
            i = e < 0 ? text.length : e + 1;
            continue;
          }
          i++;
        }
        return i + 1;
      }
      return -1;
    }
    // Index of the bracket closing the one at `open`, or -1.
    function closeOf(text, open) {
      let depth = 0;
      for (let i = open; i < text.length;) {
        const skipped = skipToken(text, i);
        if (skipped >= 0) { i = skipped; continue; }
        const c = text[i];
        if (c === '(' || c === '[' || c === '{') depth++;
        else if ((c === ')' || c === ']' || c === '}') && --depth === 0) return i;
        i++;
      }
      return -1;
    }
    // Top-level statement boundaries (start, `;`, a newline ASI may end a
    // statement at, or a closing `}`); null when there is a top-level return.
    function boundaries(text) {
      const cuts = [0];
      for (let i = 0; i < text.length;) {
        const skipped = skipToken(text, i);
        if (skipped >= 0) { i = skipped; continue; }
        const c = text[i];
        if (c === '(' || c === '[' || c === '{') {
          const e = closeOf(text, i);
          if (e < 0) return null;
          i = e + 1;
          if (c === '{') cuts.push(i);
          continue;
        }
        if (c === ';' || c === '\n') cuts.push(i + 1);
        else if (!ident(text[i - 1]) && text.startsWith('return', i) && !ident(text[i + 6])) return null;
        i++;
      }
      return cuts;
    }
    function returningList(text) {
      const cuts = boundaries(text);
      if (!cuts) return null;
      for (let k = cuts.length - 1; k >= 0; k--) {
        const head = text.slice(0, cuts[k]);
        const tail = text.slice(cuts[k]).trim().replace(/;+\s*$/, '').trim();
        // Only a `;` surely ends a statement; after a newline or `}` a tail
        // that could continue the expression is not split off.
        if (!tail || (k > 0 && text[cuts[k] - 1] !== ';' && /^[([`+\-*/%.,?:=<>&|!]/.test(tail))) continue;
        if (/^(in|instanceof|of|else|catch|finally)\b/.test(tail)) continue;
        const statement = returningStatement(tail, head);
        if (statement === null) continue;
        const code = head + '\n' + statement;
        if (compiles(code)) return code;
      }
      return null;
    }
    const returningBody = (inner) => (returningList(inner) ?? inner) + '\n';
    function returningStatement(stmt, head) {
      if (!/^(try\b|if\b|\{)/.test(stmt)) return 'return (' + stmt + '\n);';
      // A block right after `for (...)`, `else` or a label belongs to it:
      // only split where the head is a complete statement list on its own.
      if (head.trim() && head !== ';' && !compiles(head)) return null;
      let i = 0;
      const ws = () => {
        for (;;) {
          if (/\s/.test(stmt[i] || '')) { i++; continue; }
          if (stmt[i] === '/' && (stmt[i + 1] === '/' || stmt[i + 1] === '*')) { i = skipToken(stmt, i); continue; }
          return;
        }
      };
      const keyword = (word) => stmt.startsWith(word, i) && !ident(stmt[i + word.length]);
      const block = () => {
        if (stmt[i] !== '{') return null;
        const e = closeOf(stmt, i);
        if (e < 0) return null;
        const inner = stmt.slice(i + 1, e);
        i = e + 1;
        ws();
        return inner;
      };
      if (stmt[0] === '{') {
        const inner = block();
        return inner !== null && i >= stmt.length ? '{' + returningBody(inner) + '}' : null;
      }
      if (keyword('try')) {
        i += 3;
        ws();
        const tried = block();
        if (tried === null) return null;
        let out = 'try {' + returningBody(tried) + '}';
        if (keyword('catch')) {
          i += 5;
          ws();
          let param = '';
          if (stmt[i] === '(') {
            const e = closeOf(stmt, i);
            if (e < 0) return null;
            param = stmt.slice(i, e + 1);
            i = e + 1;
            ws();
          }
          const caught = block();
          if (caught === null) return null;
          out += ' catch ' + param + ' {' + returningBody(caught) + '}';
        }
        if (keyword('finally')) {
          i += 7;
          ws();
          const finalizer = block();
          if (finalizer === null) return null;
          out += ' finally {' + finalizer + '}';
        }
        return i >= stmt.length ? out : null;
      }
      i += 2;
      ws();
      if (stmt[i] !== '(') return null;
      const e = closeOf(stmt, i);
      if (e < 0) return null;
      let out = 'if ' + stmt.slice(i, e + 1);
      i = e + 1;
      ws();
      const consequent = block();
      if (consequent === null) return null;
      out += ' {' + returningBody(consequent) + '}';
      if (i >= stmt.length) return out;
      if (!keyword('else')) return null;
      const alternate = returningStatement(stmt.slice(i + 4).trim().replace(/;+\s*$/, '').trim(), ';');
      return alternate === null ? null : out + ' else ' + alternate;
    }
    const code = returningList(src);
    return code === null ? null : new AsyncFn(code);
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
