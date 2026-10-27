// Runs a caller's `execute_js` script with devtools-like semantics and a
// page-side deadline.
// 1. An expression keeps its historical meaning: `return (<script>)`.
// 2. Otherwise a statement list that compiles as script code (no top-level
//    `return`/`await`) runs through a block-scoped indirect eval so its value
//    is the last statement's completion value, e.g. `const a = 1; a + 1` is 2.
//    The syntax check compiles without running, so nothing executes twice.
// 3. Otherwise the script is an async function body (`await`, `return`).
// A promise still pending at the deadline rejects with `timeoutMark` so the
// bridge reports a clear timeout plus the page's effect evidence instead of a
// transport timeout with an unknown outcome.
(function connectorEval(source, deadlineMs, timeoutMark) {
  'use strict';
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  let run;
  try {
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
      run = () => body();
    }
  }
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(timeoutMark + ': script still pending after ' + deadlineMs + ' ms')),
      deadlineMs
    );
  });
  return Promise.race([Promise.resolve().then(run), deadline]).finally(() => clearTimeout(timer));
})
