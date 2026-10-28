// Installed at document start in the top frame of every document, before
// page scripts run and before the bridge is injected at load finish.
// 1. Page state the plugin's dispatch gate reads: a per-document epoch and
//    the time the document began unloading (beforeunload/pagehide). A
//    command delivered to a document that is unloading is refused before it
//    runs, so a reload is never answered by the old page.
// 2. An early console buffer: messages logged before the bridge exists
//    (e.g. "[HMR] connected" right after a reload) are formatted here and
//    drained by the bridge, which then sets `logs` to null to stop capture.
(function connectorEarly(formatConsole) {
  'use strict';
  if (typeof window === 'undefined' || window.top !== window) return;
  if (window.__CONNECTOR_PAGE_STATE__) return;
  const MAX_EARLY_LOGS = 500;
  const state = {
    epoch: Date.now().toString(36) + '-' + Math.random().toString(36).slice(2),
    unloadingAt: 0,
    logs: [],
    dropped: 0
  };
  try {
    Object.defineProperty(window, '__CONNECTOR_PAGE_STATE__', { value: state, configurable: true });
  } catch (_) { return; }
  const unloading = function () { state.unloadingAt = Date.now(); };
  window.addEventListener('beforeunload', unloading, true);
  window.addEventListener('pagehide', unloading, true);
  window.addEventListener('pageshow', function (e) { if (e && e.persisted) state.unloadingAt = 0; }, true);
  const c = window.console;
  if (!c) return;
  ['log', 'warn', 'error', 'info', 'debug'].forEach(function (level) {
    const original = c[level];
    if (typeof original !== 'function') return;
    c[level] = function () {
      const logs = state.logs;
      if (logs) {
        let message;
        try { message = formatConsole(arguments); }
        catch (_) { message = Array.prototype.map.call(arguments, String).join(' '); }
        logs.push({ level: level, message: message, timestamp: Date.now() });
        if (logs.length > MAX_EARLY_LOGS) { logs.shift(); state.dropped++; }
      }
      return original.apply(this, arguments);
    };
  });
})
