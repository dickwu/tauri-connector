(function connectorIpcMonitor(expectedWindow, desired, epoch) {
  const windowId = window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label;
  if (windowId !== expectedWindow) return { code: 'observation_failed', error: 'Cannot acknowledge IPC monitoring in the requested window', windowId: windowId || null };
  window.__CONNECTOR_MONITOR_PAGE_EPOCH__ ||= epoch;
  window.__CONNECTOR_IPC_MONITOR__ = desired;
  return { windowId, applied: window.__CONNECTOR_IPC_MONITOR__ === true, pageEpoch: window.__CONNECTOR_MONITOR_PAGE_EPOCH__ };
})
