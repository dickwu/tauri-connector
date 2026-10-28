// Installed at document start in the top frame of every document, before
// page scripts run and before the bridge is injected at load finish.
// 1. Page state the plugin's dispatch gate reads: a per-document epoch and
//    the time the document began unloading (beforeunload/pagehide). A
//    command delivered to a document that is unloading is refused before it
//    runs, so a reload is never answered by the old page. `navRequestedAt`
//    marks a cross-document navigation a script has requested but that has
//    not reached `beforeunload` yet (git-strata #8: `location.reload()` then
//    an immediate eval was answered by the old page).
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
    navRequestedAt: 0,
    logs: [],
    dropped: 0
  };
  try {
    Object.defineProperty(window, '__CONNECTOR_PAGE_STATE__', { value: state, configurable: true });
  } catch (_) { return; }
  const unloading = function () { state.unloadingAt = Date.now(); };
  window.addEventListener('beforeunload', unloading, true);
  window.addEventListener('pagehide', unloading, true);
  window.addEventListener('pageshow', function (e) { if (e && e.persisted) { state.unloadingAt = 0; state.navRequestedAt = 0; } }, true);
  const requested = function () { state.navRequestedAt = Date.now(); };
  // Navigation API (where available) reports every cross-document
  // navigation synchronously, before unload; an app that intercepts or
  // cancels it keeps the document, so check after its listeners ran.
  try {
    const nav = window.navigation;
    if (nav && typeof nav.addEventListener === 'function') {
      nav.addEventListener('navigate', function (e) {
        if (!e || (e.destination && e.destination.sameDocument)) return;
        Promise.resolve().then(function () {
          if (!e.defaultPrevented && !nav.transition) requested();
        });
      });
    }
  } catch (_) {}
  // Form submissions that navigate this document (no API above): a submit
  // event nobody prevented, or a direct form.submit() (fires no event).
  const sameTarget = function (form, submitter) {
    const t = (submitter && submitter.getAttribute && submitter.getAttribute('formtarget')) || (form && form.getAttribute && form.getAttribute('target')) || '';
    return t === '' || t === '_self' || t === '_top' || t === '_parent';
  };
  window.addEventListener('submit', function (e) {
    if (!e.defaultPrevented && sameTarget(e.target, e.submitter)) requested();
  });
  try {
    const proto = window.HTMLFormElement && window.HTMLFormElement.prototype;
    const submit = proto && proto.submit;
    if (typeof submit === 'function') {
      proto.submit = function () {
        if (sameTarget(this, null)) requested();
        return submit.apply(this, arguments);
      };
    }
  } catch (_) {}
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
