// Formats console arguments the way devtools does: printf-style %s %d %i %f
// %o %O substitution, `%c` styling dropped together with its CSS argument,
// and Error objects serialised as "Name: message" plus stack instead of `{}`.
(function connectorFormatConsole(args) {
  'use strict';
  const isError = v => v instanceof Error || (v && typeof v === 'object' && typeof v.message === 'string' && typeof v.stack === 'string');
  const errorText = e => {
    const head = (e.name || 'Error') + ': ' + e.message;
    const stack = typeof e.stack === 'string' ? e.stack : '';
    if (!stack) return head;
    return stack.indexOf(head) === 0 ? stack : head + '\n' + stack;
  };
  const show = v => {
    if (typeof v === 'string') return v;
    if (v === undefined) return 'undefined';
    if (typeof v === 'bigint') return v + 'n';
    if (typeof v === 'symbol' || typeof v === 'function') return String(v);
    if (isError(v)) return errorText(v);
    try {
      const seen = new WeakSet();
      const json = JSON.stringify(v, (key, value) => {
        if (typeof value === 'bigint') return value + 'n';
        if (isError(value)) return { name: value.name, message: value.message, stack: value.stack };
        if (value && typeof value === 'object') {
          if (seen.has(value)) return '[Circular]';
          seen.add(value);
        }
        return value;
      });
      return json === undefined ? String(v) : json;
    } catch (_) {
      try { return String(v); } catch (_) { return '[unserializable]'; }
    }
  };
  const list = Array.from(args);
  let index = 0;
  const parts = [];
  if (typeof list[0] === 'string' && list[0].indexOf('%') !== -1) {
    index = 1;
    parts.push(list[0].replace(/%([sdifoOc%])/g, (match, kind) => {
      if (kind === '%') return '%';
      if (index >= list.length) return match;
      const value = list[index++];
      switch (kind) {
        case 'c': return '';
        case 's': return show(value);
        case 'd':
        case 'i': return typeof value === 'symbol' ? 'NaN' : String(Math.trunc(Number(value)));
        case 'f': return typeof value === 'symbol' ? 'NaN' : String(Number(value));
        default: return show(value);
      }
    }));
  }
  for (; index < list.length; index++) parts.push(show(list[index]));
  return parts.join(' ');
})
