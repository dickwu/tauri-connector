// Effect observation for connector-run scripts.
//
// A rejected script may be retried only when it provably did not reach the
// backend. Count every backend-capable dispatch: Tauri IPC (custom protocol
// fetch or postMessage) except connector traffic, unsafe-method fetch/XHR,
// beacons and app WebSocket sends. A channel that cannot be observed reports
// "possible", which keeps the host's quarantine. Coverage is page-level: any
// dispatch while the script runs counts, including the application's own.
(function installEffects(root, SocketType, isBridgeSocket) {
  'use strict';
  let dispatches = 0, postMessageCovered = false, ipcFallback = false;
  const connectorIpc = function(command) {
    return typeof command === 'string' && command.indexOf('plugin:connector|') === 0;
  };
  const safeMethod = function(method) {
    const m = String(method || 'GET').toUpperCase();
    return m === 'GET' || m === 'HEAD' || m === 'OPTIONS';
  };
  // Returns null when the channel is absent, false when it cannot be observed.
  function hook(object, key, wrap) {
    try {
      const original = object && object[key];
      if (typeof original !== 'function') return null;
      const wrapped = wrap(original);
      object[key] = wrapped;
      return object[key] === wrapped;
    } catch (_) {
      return false;
    }
  }
  function ipcCommand(url) {
    const match = /^(?:ipc:\/\/localhost|https?:\/\/ipc\.localhost)\/([^?#]*)/.exec(url);
    if (!match) return null;
    try { return decodeURIComponent(match[1]); } catch (_) { return ''; }
  }
  const hooks = [];
  hooks.push(hook(root, 'fetch', function(original) {
    return function(input, init) {
      let command = null;
      try {
        const url = typeof input === 'string' ? input : String((input && input.url) || input);
        const method = (init && init.method) || (input && typeof input === 'object' && input.method);
        command = ipcCommand(url);
        if (command !== null ? !connectorIpc(command) : !safeMethod(method)) dispatches++;
      } catch (_) {
        dispatches++;
      }
      const result = original.apply(this, arguments);
      if (command !== null && result && typeof result.then === 'function') {
        // Tauri then retries this IPC, and later ones, through postMessage.
        result.then(null, function() { ipcFallback = true; });
      }
      return result;
    };
  }));
  const xhr = root.XMLHttpRequest && root.XMLHttpRequest.prototype;
  const xhrMethods = new WeakMap();
  hooks.push(hook(xhr, 'open', function(original) {
    return function(method) {
      try { xhrMethods.set(this, method); } catch (_) {}
      return original.apply(this, arguments);
    };
  }));
  hooks.push(hook(xhr, 'send', function(original) {
    return function() {
      if (!safeMethod(xhrMethods.get(this))) dispatches++;
      return original.apply(this, arguments);
    };
  }));
  hooks.push(hook(root.navigator, 'sendBeacon', function(original) {
    return function() {
      dispatches++;
      return original.apply(this, arguments);
    };
  }));
  hooks.push(hook(SocketType && SocketType.prototype, 'send', function(original) {
    return function() {
      if (!isBridgeSocket(this)) dispatches++;
      return original.apply(this, arguments);
    };
  }));
  const covered = hooks.every(function(result) { return result !== false; });
  postMessageCovered = hook(root.ipc, 'postMessage', function(original) {
    return function(message) {
      let command = null;
      try {
        if (typeof message === 'string' && message.length <= 65536) command = JSON.parse(message).cmd;
      } catch (_) {}
      if (!connectorIpc(command)) dispatches++;
      return original.apply(this, arguments);
    };
  }) === true;
  // Android always uses postMessage; elsewhere it is only Tauri's fallback.
  const agent = (root.navigator && root.navigator.userAgent) || '';
  const postMessageRequired = /Android/i.test(agent);
  return Object.freeze({
    mark: function() { return dispatches; },
    unchangedSince: function(mark) {
      const ipcObserved = postMessageCovered || (!postMessageRequired && !ipcFallback);
      return covered && ipcObserved && dispatches === mark;
    }
  });
})
