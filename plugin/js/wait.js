(function connectorWait(check, timeout) {
  return new Promise(resolve => {
    const start = Date.now();
    let stopped = false;
    let pollTimer;
    let deadlineTimer;
    let lastObservation = null;
    const finish = result => {
      if (stopped) return;
      stopped = true;
      clearTimeout(pollTimer);
      clearTimeout(deadlineTimer);
      window.removeEventListener('pagehide', onPageHide);
      resolve({ ...result, elapsed_ms: Date.now() - start, lastObservation });
    };
    const onPageHide = () => finish({ found: false, code: 'observation_failed', error: 'Page navigated while waiting' });
    window.addEventListener('pagehide', onPageHide, { once: true });
    deadlineTimer = setTimeout(() => finish({ found: false, timeout: true }), Math.max(0, timeout));
    const poll = async () => {
      if (stopped) return;
      try {
        const found = !!(await check());
        if (stopped) return;
        lastObservation = found;
        if (found) finish({ found: true });
        else pollTimer = setTimeout(poll, Math.min(100, Math.max(0, timeout - (Date.now() - start))));
      } catch (error) {
        finish({ found: false, code: 'observation_failed', error: String(error && error.message || error) });
      }
    };
    poll();
  });
})
