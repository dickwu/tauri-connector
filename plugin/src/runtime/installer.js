// Receives only host-embedded modules. There is no public module registration.
((configuration, modules) => {
  'use strict';
  const bootstrap = window.__CONNECTOR_BOOTSTRAP__;
  const context = Object.freeze(configuration.context);
  let disposed = false, cleanupFailed = false, cleanupPending = 0;
  const cleanupConfirmed = () => !cleanupFailed && cleanupPending === 0 && Object.values(modules).every(module => (module.activeObservers?.() || 0) === 0);
  let unsubscribe = () => {};
  const rejection = code => ({ ok: false, dispatched: false, phase: 'pre_dispatch', error: {code, stage:'preparing', message:code, retryableBeforeDispatch:false} });
  const runtime = Object.freeze({
    context, runtimeProtocolVersion: 1, runtimeVersion: configuration.runtimeVersion,
    semanticVersion: configuration.semanticVersion, bundleHash: configuration.bundleHash,
    modules: Object.freeze(Object.keys(modules)),
    status() { return {ready:!disposed && !bootstrap.suspended && bootstrap.pageEpoch === context.pageEpoch,
      context, runtimeProtocolVersion:1,runtimeVersion:configuration.runtimeVersion,semanticVersion:configuration.semanticVersion,
      bundleHash:configuration.bundleHash,modules:Object.keys(modules),inputGuard:bootstrap.guard.state(),activeObservers:Object.values(modules).reduce((count,module)=>count+(module.activeObservers?.()||0),0)}; },
    async dispatch(packet) {
      if (disposed || bootstrap.suspended || bootstrap.pageEpoch !== context.pageEpoch) return rejection('stale_context');
      if (packet.runtimeProtocolVersion !== 1) return rejection('runtime_protocol_mismatch');
      if (!packet.expectedContext || Object.keys(context).some(key => packet.expectedContext[key] !== context[key])) return rejection('stale_context');
      if (!Number.isFinite(packet.remainingMs) || packet.remainingMs <= 0) return rejection('deadline_expired');
      if (!Object.hasOwn(modules,packet.module) || packet.module === 'semantic') return rejection('unsupported_module');
      if (typeof packet.requestId !== 'string' || !packet.requestId) return rejection('invalid_request');
      for (const expected of [packet.args?.context, packet.args?.expectedContext]) {
        if (expected && Object.keys(context).some(key => Object.hasOwn(expected,key) && expected[key] !== context[key])) return rejection('stale_context');
      }
      const module = modules[packet.module];
      const dispatch = typeof module === 'function' ? module : module.dispatch;
      if (typeof dispatch !== 'function') return rejection('unsupported_operation');
      const args = {...packet.args, context: {...packet.args?.context,...context}, remainingMs:packet.remainingMs};
      if(packet.module==='workflow' && Number.isFinite(args.timeoutMs)) args.timeoutMs=Math.min(args.timeoutMs,packet.remainingMs);
      // This call is the only business dispatch. Errors after this line never
      // authorize the host to replay the operation, regardless of their text.
      const result = await dispatch.call(module,args);
      if (result && typeof result === 'object' && result.context) result.context = {...result.context,...context};
      return result;
    },
    dispose(reason = 'host_dispose') {
      if (disposed) return {cleaned:cleanupConfirmed()};
      disposed = true;
      for (const module of Object.values(modules).reverse()) {
        try {
          const result=module.dispose?.(reason);
          if(result && typeof result.then==='function') {
            cleanupPending++;
            Promise.resolve(result).then(value=>{if(value?.confirmed===false || value?.cleaned===false)cleanupFailed=true;},()=>{cleanupFailed=true;}).finally(()=>{cleanupPending--;});
          } else if(result?.confirmed===false || result?.cleaned===false) cleanupFailed=true;
        }
        catch (_) { cleanupFailed=true; }
      }
      unsubscribe();
      return {cleaned:cleanupConfirmed()};
    }
  });
  unsubscribe = bootstrap.registerLifecycle(reason=>runtime.dispose(reason));
  window.__CONNECTOR_INSPECTION_RUNTIME__ = runtime;
  return runtime.status();
})
