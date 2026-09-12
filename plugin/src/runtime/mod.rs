mod manager;
pub mod manifest;

pub use manager::{RuntimeManager, RuntimeTarget, remaining};

pub fn initialization_script() -> &'static str {
    manifest::BOOTSTRAP
}

pub fn json(value: &serde_json::Value) -> String {
    value
        .to_string()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub fn json_expression(value: &serde_json::Value) -> String {
    // JSON.parse preserves own __proto__/constructor keys; inserting a JSON
    // object directly as a JS object literal changes __proto__ semantics.
    format!(
        "JSON.parse({})",
        json(&serde_json::Value::String(json(value)))
    )
}

pub fn dispatch_script(packet: &serde_json::Value) -> String {
    format!(
        "/*__CONNECTOR_RUNTIME_COMMAND__*/(window.__CONNECTOR_INSPECTION_RUNTIME__?window.__CONNECTOR_INSPECTION_RUNTIME__.dispatch({}):({{ok:false,dispatched:false,phase:'pre_dispatch',error:{{code:'runtime_missing',stage:'preparing',message:'runtime_missing'}}}}))",
        json_expression(packet)
    )
}

pub fn install_script(configuration: &serde_json::Value) -> String {
    // Only values use JSON. All executable source comes from the static manifest.
    format!(
        r#"/*__CONNECTOR_INSTALL_BUNDLE__*/(()=>{{
      const c={}; const b=window.__CONNECTOR_BOOTSTRAP__;
      if(window.top!==window || !b || b.suspended || b.pageEpoch!==c.context.pageEpoch || location.origin!==c.origin) return {{ready:false,error:'stale_context'}};
      const previous=window.__CONNECTOR_INSPECTION_RUNTIME__;
      if(previous && previous.status().ready) {{
        if(previous.bundleHash===c.bundleHash && JSON.stringify(previous.context)===JSON.stringify(c.context)) return previous.status();
        return {{ready:false,error:'runtime_replacement_requires_reload'}};
      }}
      if(!previous && window.__TAURI_CONNECTOR_WORKFLOW_V1__) return {{ready:false,error:'runtime_cleanup_unproven'}};
      if(previous && !previous.dispose('replace').cleaned) return {{ready:false,error:'runtime_cleanup_unproven'}};
      const modules=Object.create(null);
      modules.semantic=({}); modules.workflow=({}); modules.picker=({}); modules.ipcCapture=({})(window); modules.geometry=({});
      return ({}) (c,modules);
    }})()"#,
        json_expression(configuration),
        manifest::SEMANTIC,
        manifest::WORKFLOW,
        manifest::PICKER,
        manifest::CAPTURE,
        manifest::GEOMETRY,
        manifest::INSTALLER
    )
}

pub const PROBE: &str = "/*__CONNECTOR_RUNTIME_COMMAND__*/(()=>{if(window.top!==window)return {available:false,reason:'subframe'};const b=window.__CONNECTOR_BOOTSTRAP__;return {available:!!b,pageEpoch:b?.pageEpoch,suspended:b?.suspended,origin:location.origin,url:String(location.href),runtime:window.__CONNECTOR_INSPECTION_RUNTIME__?.status()||null}})()";
