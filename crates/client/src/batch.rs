//! Generic batch-action executor.
//!
//! Parses a JSON spec describing a list of MCP tool actions, runs them
//! sequentially, in parallel, or DAG-ordered via `dependsOn`, and produces a
//! structured run report with one log entry per action. The executor is
//! generic over an async dispatcher, so the CLI, the standalone MCP server,
//! and the plugin's embedded MCP server all share the same scheduling logic
//! while dispatching through their own tool tables.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::outcome::{legacy_outcome, EffectStatus, ExecutionOutcome, WorkflowError};
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Execution mode for a batch when no explicit dependencies decide the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BatchMode {
    /// Run actions one after another in spec order (each action implicitly
    /// depends on the previous one).
    #[default]
    Sequential,
    /// Start all actions concurrently; only explicit `dependsOn` edges order
    /// them.
    Parallel,
}

/// One action inside a batch: an MCP tool name plus its arguments.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionSpec {
    /// Optional stable id, referenced by `dependsOn` of other actions.
    #[serde(default)]
    pub id: Option<String>,
    /// MCP tool name (e.g. `webview_interact`, `webview_execute_js`).
    pub tool: String,
    /// Tool arguments object (same shape as a direct MCP tool call).
    #[serde(default)]
    pub args: Value,
    /// Ids of actions that must succeed before this one starts.
    #[serde(default, alias = "depends_on")]
    pub depends_on: Vec<String>,
    /// Per-action timeout override in milliseconds.
    #[serde(default, alias = "timeout_ms")]
    pub timeout_ms: Option<u64>,
    /// Omit the tool result from the log entry (status/timing only).
    #[serde(default, alias = "omit_result")]
    pub omit_result: bool,
}

/// Parsed batch specification.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchSpec {
    #[serde(default)]
    pub mode: BatchMode,
    /// Stop starting new actions after the first failure (default true).
    /// In-flight actions still finish; unstarted ones are logged as skipped.
    #[serde(default = "default_true", alias = "stop_on_error")]
    pub stop_on_error: bool,
    /// Maximum number of concurrently running actions (default unlimited).
    #[serde(default, alias = "max_parallel")]
    pub max_parallel: Option<usize>,
    /// Default per-action timeout in milliseconds.
    #[serde(default, alias = "timeout_ms")]
    pub timeout_ms: Option<u64>,
    /// Write the run report as pretty JSON to this file path.
    #[serde(default)]
    pub save: Option<String>,
    pub actions: Vec<ActionSpec>,
}

fn default_true() -> bool {
    true
}

impl BatchSpec {
    /// Parse a spec from JSON. Accepts either the full object form or a bare
    /// array of actions (shorthand for `{ "actions": [...] }`).
    pub fn parse(value: &Value) -> Result<Self, String> {
        let object = match value {
            Value::Array(actions) => {
                serde_json::json!({ "actions": actions })
            }
            Value::Object(_) => value.clone(),
            _ => return Err("batch spec must be a JSON object or array".to_string()),
        };
        let spec: BatchSpec =
            serde_json::from_value(object).map_err(|e| format!("Invalid batch spec: {e}"))?;
        spec.validate()?;
        Ok(spec)
    }

    fn validate(&self) -> Result<(), String> {
        if self.actions.is_empty() {
            return Err("batch spec has no actions".to_string());
        }
        let mut ids: HashSet<&str> = HashSet::new();
        for action in &self.actions {
            if action.tool.trim().is_empty() {
                return Err("every action needs a non-empty 'tool'".to_string());
            }
            if !matches!(action.args, Value::Object(_) | Value::Null) {
                return Err(format!(
                    "action '{}': 'args' must be a JSON object",
                    action.id.as_deref().unwrap_or(&action.tool)
                ));
            }
            if let Some(id) = &action.id {
                if !ids.insert(id.as_str()) {
                    return Err(format!("duplicate action id '{id}'"));
                }
            }
        }
        for action in &self.actions {
            for dep in &action.depends_on {
                if !ids.contains(dep.as_str()) {
                    return Err(format!(
                        "action '{}' depends on unknown id '{dep}'",
                        action.id.as_deref().unwrap_or(&action.tool)
                    ));
                }
                if action.id.as_deref() == Some(dep.as_str()) {
                    return Err(format!("action '{dep}' depends on itself"));
                }
            }
        }

        // Reject dependency cycles up front (Kahn over the same edge set the
        // scheduler uses: explicit dependsOn plus the sequential chain), so a
        // bad spec fails with a clear error instead of a report full of
        // never-became-ready skips.
        let (deps, _) = self.dependency_edges();
        let n = self.actions.len();
        let mut remaining: Vec<usize> = deps.iter().map(Vec::len).collect();
        let mut queue: VecDeque<usize> = (0..n).filter(|&i| remaining[i] == 0).collect();
        let mut resolved = 0usize;
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, dep_list) in deps.iter().enumerate() {
            for &d in dep_list {
                dependents[d].push(i);
            }
        }
        while let Some(i) = queue.pop_front() {
            resolved += 1;
            for &d in &dependents[i] {
                remaining[d] -= 1;
                if remaining[d] == 0 {
                    queue.push_back(d);
                }
            }
        }
        if resolved < n {
            let stuck: Vec<&str> = (0..n)
                .filter(|&i| remaining[i] > 0)
                .filter_map(|i| self.actions[i].id.as_deref())
                .collect();
            return Err(format!("dependency cycle among actions: {stuck:?}"));
        }
        Ok(())
    }

    /// Build the scheduling edges: `(all_deps, explicit_deps)` per action.
    /// `all_deps` adds the implicit previous-action chain in sequential mode;
    /// `explicit_deps` holds only user-written `dependsOn` edges (the ones a
    /// failure propagates along).
    fn dependency_edges(&self) -> (Vec<Vec<usize>>, Vec<Vec<usize>>) {
        let n = self.actions.len();
        let id_to_index: HashMap<&str, usize> = self
            .actions
            .iter()
            .enumerate()
            .filter_map(|(i, a)| a.id.as_deref().map(|id| (id, i)))
            .collect();
        let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut explicit: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, action) in self.actions.iter().enumerate() {
            for dep in &action.depends_on {
                // Unknown ids are rejected by validate(); skip defensively for
                // callers constructing a BatchSpec by hand.
                let Some(&di) = id_to_index.get(dep.as_str()) else {
                    continue;
                };
                if !deps[i].contains(&di) {
                    deps[i].push(di);
                }
                if !explicit[i].contains(&di) {
                    explicit[i].push(di);
                }
            }
            if self.mode == BatchMode::Sequential && i > 0 && !deps[i].contains(&(i - 1)) {
                deps[i].push(i - 1);
            }
        }
        (deps, explicit)
    }
}

/// Legacy compatibility belongs only to tools whose published contract uses
/// a top-level error string. Query/eval data is never interpreted as an error.
fn adapt_legacy(tool: &str, result: Result<Value, String>) -> ExecutionOutcome {
    match result {
        Err(message) => ExecutionOutcome::unknown(WorkflowError::new(
            "outcome_unknown",
            "dispatching",
            message,
        )),
        Ok(value) => {
            if matches!(
                tool,
                "webview_interact"
                    | "webview_keyboard"
                    | "webview_wait_for"
                    | "webview_act_and_verify"
                    | "webview_locator"
                    | "webview_select_option"
                    | "webview_scroll"
            ) {
                if let Some(message) = value.get("error").and_then(Value::as_str) {
                    let mut outcome = ExecutionOutcome::failed(
                        WorkflowError::new(
                            "execution_failed",
                            "executing",
                            format!("{message} — {value}"),
                        ),
                        EffectStatus::Possible,
                    );
                    outcome.data = value;
                    return outcome;
                }
            }
            legacy_outcome(tool, value)
        }
    }
}

/// Batch screenshots use durable references by default. The legacy explicit
/// `save:false` opt-out remains available to callers needing inline images.
pub fn prepare_tool_args(tool: &str, mut args: Value) -> Value {
    if tool == "webview_screenshot" {
        if args.is_null() {
            args = serde_json::json!({});
        }
        if let Some(object) = args.as_object_mut() {
            object.entry("save").or_insert(Value::Bool(true));
        }
    }
    args
}

/// Compact only a known screenshot result that provides a saved artifact.
/// Never strip image-looking fields from arbitrary business query values.
pub fn compact_tool_outcome(tool: &str, outcome: &mut ExecutionOutcome) {
    if tool != "webview_screenshot" {
        return;
    }
    let Some(artifact) = outcome.data.get("artifact") else {
        return;
    };
    let Some(id) = artifact
        .get("artifactId")
        .or_else(|| artifact.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
    else {
        return;
    };
    if !artifact
        .get("path")
        .and_then(Value::as_str)
        .is_some_and(|path| !path.trim().is_empty())
    {
        return;
    }
    let id = id.to_string();
    if let Some(data) = outcome.data.as_object_mut() {
        data.remove("base64");
    }
    if !outcome.evidence_refs.contains(&id) {
        outcome.evidence_refs.push(id);
    }
}

/// Per-action run log entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionLog {
    pub index: usize,
    pub id: String,
    pub tool: String,
    /// "ok" | "error" | "skipped"
    pub status: String,
    /// Offset from batch start when the action began, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ExecutionOutcome>,
}

/// Full batch run report returned to the caller (and optionally saved to disk).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReport {
    pub ok: bool,
    pub mode: BatchMode,
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Unix epoch milliseconds when the batch started.
    pub started_at: u64,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistence_warning: Option<String>,
    pub logs: Vec<ActionLog>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ActionState {
    Pending,
    Running,
    Ok,
    Failed,
    Skipped,
}

/// Parse a spec value, run it through `dispatch`, and save the report when the
/// spec requests it. This is the single entry point shared by the CLI and both
/// MCP servers.
pub async fn run_from_value<F, Fut>(spec_value: &Value, dispatch: F) -> Result<BatchReport, String>
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let spec = BatchSpec::parse(spec_value)?;
    let report = run_batch(&spec, dispatch).await;
    Ok(persist_report(&spec, report))
}

/// Typed entry point for internal dispatchers, without MCP envelope round trips.
pub async fn run_from_value_outcomes<F, Fut>(
    spec_value: &Value,
    dispatch: F,
) -> Result<BatchReport, String>
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = ExecutionOutcome>,
{
    let spec = BatchSpec::parse(spec_value)?;
    let report = run_batch_outcomes(&spec, dispatch).await;
    Ok(persist_report(&spec, report))
}

fn persist_report(spec: &BatchSpec, mut report: BatchReport) -> BatchReport {
    if let Some(path) = &spec.save {
        // Include the destination before serialization; clear it on failure.
        report.saved_to = Some(path.clone());
        if let Err(error) = save_report(path, &report) {
            report.saved_to = None;
            report.persistence_warning = Some(error);
        }
    }
    report
}

/// Run a parsed batch spec through `dispatch`, returning the report.
pub async fn run_batch<F, Fut>(spec: &BatchSpec, dispatch: F) -> BatchReport
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    run_batch_outcomes(spec, |tool, args| {
        let future = dispatch(tool.clone(), args);
        async move { adapt_legacy(&tool, future.await) }
    })
    .await
}

/// Shared scheduler over typed outcomes. Failed verification and unknown
/// execution block explicit dependencies exactly like execution errors.
pub async fn run_batch_outcomes<F, Fut>(spec: &BatchSpec, dispatch: F) -> BatchReport
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = ExecutionOutcome>,
{
    let n = spec.actions.len();
    let started_at = unix_ms();
    let start = Instant::now();

    // Scheduling edges (with the sequential chain) vs explicit dependsOn
    // edges. Readiness follows `deps`; failures propagate only along
    // `explicit_deps`, so sequential + stopOnError:false still runs the rest
    // in order after a failure.
    let (deps, explicit_deps) = spec.dependency_edges();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut remaining: Vec<usize> = vec![0; n];
    for (i, dep_list) in deps.iter().enumerate() {
        remaining[i] = dep_list.len();
        for &d in dep_list {
            dependents[d].push(i);
        }
    }

    let mut state: Vec<ActionState> = vec![ActionState::Pending; n];
    let mut logs: Vec<Option<ActionLog>> = (0..n).map(|_| None).collect();
    let mut ready: VecDeque<usize> = (0..n).filter(|&i| remaining[i] == 0).collect();
    let mut abort = false;

    let max_parallel = spec.max_parallel.unwrap_or(usize::MAX).max(1);
    let mut running = FuturesUnordered::new();

    // Display ids: explicit id, or a synthesized `a{index}` nudged with `_`
    // prefixes until it collides with no explicit id.
    let explicit_ids: HashSet<&str> = spec
        .actions
        .iter()
        .filter_map(|a| a.id.as_deref())
        .collect();
    let log_ids: Vec<String> = spec
        .actions
        .iter()
        .enumerate()
        .map(|(i, a)| match &a.id {
            Some(id) => id.clone(),
            None => {
                let mut candidate = format!("a{i}");
                while explicit_ids.contains(candidate.as_str()) {
                    candidate.insert(0, '_');
                }
                candidate
            }
        })
        .collect();

    // Mark an action skipped and release its dependents (which may cascade).
    #[allow(clippy::too_many_arguments)]
    fn skip_action(
        i: usize,
        reason: String,
        spec: &BatchSpec,
        log_ids: &[String],
        state: &mut [ActionState],
        logs: &mut [Option<ActionLog>],
        remaining: &mut [usize],
        dependents: &[Vec<usize>],
        newly_unblocked: &mut VecDeque<usize>,
    ) {
        state[i] = ActionState::Skipped;
        logs[i] = Some(ActionLog {
            index: i,
            id: log_ids[i].clone(),
            tool: spec.actions[i].tool.clone(),
            status: "skipped".to_string(),
            started_at_ms: None,
            duration_ms: None,
            result: None,
            error: Some(reason),
            outcome: None,
        });
        for &d in &dependents[i] {
            remaining[d] -= 1;
            if remaining[d] == 0 {
                newly_unblocked.push_back(d);
            }
        }
    }

    loop {
        // Start ready actions (or skip them when aborting / deps failed).
        while let Some(i) = ready.pop_front() {
            if state[i] != ActionState::Pending {
                continue;
            }
            let failed_dep = explicit_deps[i]
                .iter()
                .find(|&&d| matches!(state[d], ActionState::Failed | ActionState::Skipped));
            if let Some(&d) = failed_dep {
                let mut unblocked = VecDeque::new();
                let reason = match state[d] {
                    ActionState::Failed => format!("dependency '{}' failed", log_ids[d]),
                    _ => format!("dependency '{}' was skipped", log_ids[d]),
                };
                skip_action(
                    i,
                    reason,
                    spec,
                    &log_ids,
                    &mut state,
                    &mut logs,
                    &mut remaining,
                    &dependents,
                    &mut unblocked,
                );
                ready.append(&mut unblocked);
                continue;
            }
            if abort {
                let mut unblocked = VecDeque::new();
                skip_action(
                    i,
                    "batch aborted after earlier failure (stopOnError)".to_string(),
                    spec,
                    &log_ids,
                    &mut state,
                    &mut logs,
                    &mut remaining,
                    &dependents,
                    &mut unblocked,
                );
                ready.append(&mut unblocked);
                continue;
            }
            if running.len() >= max_parallel {
                ready.push_front(i);
                break;
            }

            state[i] = ActionState::Running;
            let tool = spec.actions[i].tool.clone();
            let args = match &spec.actions[i].args {
                Value::Null => Value::Object(serde_json::Map::new()),
                other => other.clone(),
            };
            let timeout_ms = spec.actions[i].timeout_ms.or(spec.timeout_ms);
            let args = prepare_tool_args(&tool, args);
            let fut = dispatch(tool, args);
            running.push(async move {
                // Stamp the start at first poll, not at schedule time, so the
                // offset reflects when the action actually began running.
                let started_at_ms = start.elapsed().as_millis() as u64;
                let action_start = Instant::now();
                let result = match timeout_ms {
                    Some(ms) => match tokio::time::timeout(Duration::from_millis(ms), fut).await {
                        Ok(r) => r,
                        Err(_) => ExecutionOutcome::unknown(WorkflowError::new(
                            "outcome_unknown",
                            "dispatching",
                            format!("action timed out after {ms}ms; remote execution may continue"),
                        )),
                    },
                    None => fut.await,
                };
                (
                    i,
                    started_at_ms,
                    action_start.elapsed().as_millis() as u64,
                    result,
                )
            });
        }

        if running.is_empty() {
            break;
        }

        // Wait for the next completion, then release its dependents.
        let Some((i, started_at_ms, duration_ms, result)) = running.next().await else {
            break;
        };
        let mut outcome = result;
        compact_tool_outcome(&spec.actions[i].tool, &mut outcome);
        let (status, ok_result, error) = if outcome.is_success(false) {
            state[i] = ActionState::Ok;
            let kept = if spec.actions[i].omit_result {
                None
            } else {
                Some(outcome.data.clone())
            };
            ("ok", kept, None)
        } else {
            state[i] = ActionState::Failed;
            if spec.stop_on_error {
                abort = true;
            }
            let message = outcome
                .error
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "Action execution or verification did not succeed".to_string());
            ("error", None, Some(message))
        };
        let mut visible_outcome = outcome;
        if spec.actions[i].omit_result {
            visible_outcome.data = Value::Null;
        }
        logs[i] = Some(ActionLog {
            index: i,
            id: log_ids[i].clone(),
            tool: spec.actions[i].tool.clone(),
            status: status.to_string(),
            started_at_ms: Some(started_at_ms),
            duration_ms: Some(duration_ms),
            result: ok_result,
            error,
            outcome: Some(visible_outcome),
        });
        for &d in &dependents[i] {
            remaining[d] -= 1;
            if remaining[d] == 0 {
                ready.push_back(d);
            }
        }
    }

    // Anything still pending (unreachable due to skip cascades) is skipped.
    for i in 0..n {
        if state[i] == ActionState::Pending {
            logs[i] = Some(ActionLog {
                index: i,
                id: log_ids[i].clone(),
                tool: spec.actions[i].tool.clone(),
                status: "skipped".to_string(),
                started_at_ms: None,
                duration_ms: None,
                result: None,
                error: Some("never became ready (dependency chain did not complete)".to_string()),
                outcome: None,
            });
            state[i] = ActionState::Skipped;
        }
    }

    let logs: Vec<ActionLog> = logs.into_iter().flatten().collect();
    let succeeded = logs.iter().filter(|l| l.status == "ok").count();
    let failed = logs.iter().filter(|l| l.status == "error").count();
    let skipped = logs.iter().filter(|l| l.status == "skipped").count();

    BatchReport {
        ok: failed == 0 && skipped == 0,
        mode: spec.mode,
        total: n,
        succeeded,
        failed,
        skipped,
        started_at,
        duration_ms: start.elapsed().as_millis() as u64,
        saved_to: None,
        persistence_warning: None,
        logs,
    }
}

/// Write a report as pretty JSON to `path`, creating parent directories.
pub fn save_report(path: &str, report: &BatchReport) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| format!("Failed to serialize batch report: {e}"))?;
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(path, json).map_err(|e| format!("Failed to write {path}: {e}"))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn spec(value: Value) -> BatchSpec {
        BatchSpec::parse(&value).expect("valid spec")
    }

    /// Dispatcher that records call order and sleeps briefly.
    fn recording_dispatch(
        calls: Arc<Mutex<Vec<String>>>,
    ) -> impl Fn(String, Value) -> std::pin::Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>
    {
        move |tool, args| {
            let calls = calls.clone();
            Box::pin(async move {
                calls.lock().await.push(tool.clone());
                tokio::time::sleep(Duration::from_millis(5)).await;
                if tool == "fail" {
                    Err("boom".to_string())
                } else {
                    Ok(json!({ "tool": tool, "args": args }))
                }
            })
        }
    }

    #[tokio::test]
    async fn sequential_runs_in_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let s = spec(json!({
            "mode": "sequential",
            "actions": [
                { "tool": "one" },
                { "tool": "two" },
                { "tool": "three" }
            ]
        }));
        let report = run_batch(&s, recording_dispatch(calls.clone())).await;
        assert!(report.ok);
        assert_eq!(report.succeeded, 3);
        assert_eq!(*calls.lock().await, vec!["one", "two", "three"]);
        // Sequential actions must not overlap: starts are ordered.
        let starts: Vec<u64> = report
            .logs
            .iter()
            .map(|l| l.started_at_ms.unwrap())
            .collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    }

    #[tokio::test]
    async fn parallel_actions_overlap() {
        // Two actions that each wait for the other to start: they only finish
        // if they truly run concurrently.
        let first = Arc::new(tokio::sync::Notify::new());
        let second = Arc::new(tokio::sync::Notify::new());
        let s = spec(json!({
            "mode": "parallel",
            "actions": [ { "tool": "a" }, { "tool": "b" } ]
        }));
        let dispatch = move |tool: String, _args: Value| {
            let first = first.clone();
            let second = second.clone();
            async move {
                if tool == "a" {
                    first.notify_one();
                    second.notified().await;
                } else {
                    second.notify_one();
                    first.notified().await;
                }
                Ok(json!(tool))
            }
        };
        let report = tokio::time::timeout(Duration::from_secs(5), run_batch(&s, dispatch))
            .await
            .expect("parallel actions deadlocked — they did not overlap");
        assert!(report.ok);
        assert_eq!(report.succeeded, 2);
    }

    #[tokio::test]
    async fn depends_on_orders_dag() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let s = spec(json!({
            "mode": "parallel",
            "actions": [
                { "id": "setup", "tool": "one" },
                { "id": "left", "tool": "two", "dependsOn": ["setup"] },
                { "id": "right", "tool": "three", "dependsOn": ["setup"] },
                { "id": "final", "tool": "four", "dependsOn": ["left", "right"] }
            ]
        }));
        let report = run_batch(&s, recording_dispatch(calls.clone())).await;
        assert!(report.ok, "report: {report:?}");
        let calls = calls.lock().await;
        assert_eq!(calls[0], "one");
        assert_eq!(calls[3], "four");
    }

    #[tokio::test]
    async fn stop_on_error_skips_rest_sequentially() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let s = spec(json!({
            "actions": [
                { "tool": "one" },
                { "tool": "fail" },
                { "tool": "three" }
            ]
        }));
        let report = run_batch(&s, recording_dispatch(calls.clone())).await;
        assert!(!report.ok);
        assert_eq!(report.succeeded, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.logs[2].status, "skipped");
        assert_eq!(*calls.lock().await, vec!["one", "fail"]);
    }

    #[tokio::test]
    async fn no_stop_on_error_continues_independent_actions() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let s = spec(json!({
            "mode": "parallel",
            "stopOnError": false,
            "actions": [
                { "id": "bad", "tool": "fail" },
                { "id": "child", "tool": "two", "dependsOn": ["bad"] },
                { "id": "free", "tool": "three" }
            ]
        }));
        let report = run_batch(&s, recording_dispatch(calls.clone())).await;
        assert!(!report.ok);
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 1); // child of the failure
        assert_eq!(report.succeeded, 1); // independent action still ran
        let child = report.logs.iter().find(|l| l.id == "child").unwrap();
        assert_eq!(child.status, "skipped");
        assert!(child
            .error
            .as_deref()
            .unwrap()
            .contains("dependency 'bad' failed"));
    }

    #[tokio::test]
    async fn per_action_timeout_fails_action() {
        let s = spec(json!({
            "actions": [ { "tool": "slow", "timeoutMs": 20 } ]
        }));
        let dispatch = |_tool: String, _args: Value| async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(json!(null))
        };
        let report = run_batch(&s, dispatch).await;
        assert!(!report.ok);
        assert!(report.logs[0]
            .error
            .as_deref()
            .unwrap()
            .contains("timed out after 20ms"));
    }

    #[tokio::test]
    async fn max_parallel_limits_concurrency() {
        let live = Arc::new(Mutex::new((0usize, 0usize))); // (current, peak)
        let s = spec(json!({
            "mode": "parallel",
            "maxParallel": 2,
            "actions": [
                { "tool": "a" }, { "tool": "b" }, { "tool": "c" }, { "tool": "d" }
            ]
        }));
        let dispatch = move |_tool: String, _args: Value| {
            let live = live.clone();
            async move {
                {
                    let mut g = live.lock().await;
                    g.0 += 1;
                    g.1 = g.1.max(g.0);
                    assert!(g.0 <= 2, "more than maxParallel actions ran at once");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
                live.lock().await.0 -= 1;
                Ok(json!(null))
            }
        };
        let report = run_batch(&s, dispatch).await;
        assert!(report.ok);
    }

    #[tokio::test]
    async fn omit_result_drops_payload() {
        let s = spec(json!({
            "actions": [ { "tool": "one", "omitResult": true } ]
        }));
        let dispatch = |_t: String, _a: Value| async move { Ok(json!({ "huge": "payload" })) };
        let report = run_batch(&s, dispatch).await;
        assert!(report.ok);
        assert_eq!(report.logs[0].status, "ok");
        assert!(report.logs[0].result.is_none());
    }

    #[tokio::test]
    async fn run_from_value_saves_report() {
        let path =
            std::env::temp_dir().join(format!("connector-batch-test-{}.json", std::process::id()));
        let path_str = path.to_string_lossy().to_string();
        let value = json!({
            "save": path_str,
            "actions": [ { "tool": "one" } ]
        });
        let dispatch = |_t: String, _a: Value| async move { Ok(json!(1)) };
        let report = run_from_value(&value, dispatch).await.unwrap();
        assert_eq!(report.saved_to.as_deref(), Some(path_str.as_str()));
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["total"], json!(1));
        assert_eq!(saved["logs"][0]["status"], json!("ok"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn bare_array_is_actions_shorthand() {
        let value = json!([ { "tool": "one" }, { "tool": "two" } ]);
        let dispatch = |_t: String, _a: Value| async move { Ok(json!(null)) };
        let report = run_from_value(&value, dispatch).await.unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.mode, BatchMode::Sequential);
    }

    #[test]
    fn validation_rejects_bad_specs() {
        // Empty actions
        assert!(BatchSpec::parse(&json!({ "actions": [] })).is_err());
        // Unknown dependency
        assert!(BatchSpec::parse(&json!({
            "actions": [ { "tool": "a", "dependsOn": ["ghost"] } ]
        }))
        .is_err());
        // Duplicate ids
        assert!(BatchSpec::parse(&json!({
            "actions": [
                { "id": "x", "tool": "a" },
                { "id": "x", "tool": "b" }
            ]
        }))
        .is_err());
        // Self dependency
        assert!(BatchSpec::parse(&json!({
            "actions": [ { "id": "x", "tool": "a", "dependsOn": ["x"] } ]
        }))
        .is_err());
        // Non-object args
        assert!(BatchSpec::parse(&json!({
            "actions": [ { "tool": "a", "args": "nope" } ]
        }))
        .is_err());
        // Snake_case aliases accepted
        let s = BatchSpec::parse(&json!({
            "stop_on_error": false,
            "actions": [ { "tool": "a", "timeout_ms": 5 } ]
        }))
        .unwrap();
        assert!(!s.stop_on_error);
        assert_eq!(s.actions[0].timeout_ms, Some(5));
    }

    #[test]
    fn validation_rejects_dependency_cycles() {
        // Explicit a <-> b cycle.
        let err = BatchSpec::parse(&json!({
            "mode": "parallel",
            "actions": [
                { "id": "a", "tool": "one", "dependsOn": ["b"] },
                { "id": "b", "tool": "two", "dependsOn": ["a"] }
            ]
        }))
        .unwrap_err();
        assert!(err.contains("cycle"), "unexpected error: {err}");

        // Sequential chain + a forward explicit dep also forms a cycle.
        let err = BatchSpec::parse(&json!({
            "actions": [
                { "id": "first", "tool": "one", "dependsOn": ["second"] },
                { "id": "second", "tool": "two" }
            ]
        }))
        .unwrap_err();
        assert!(err.contains("cycle"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn hand_built_cycle_never_deadlocks() {
        // Defense in depth: a caller constructing a cyclic BatchSpec directly
        // (bypassing parse/validate) must get skips, not a hang.
        let action = |id: &str, dep: &str| ActionSpec {
            id: Some(id.to_string()),
            tool: "one".to_string(),
            args: Value::Null,
            depends_on: vec![dep.to_string()],
            timeout_ms: None,
            omit_result: false,
        };
        let s = BatchSpec {
            mode: BatchMode::Parallel,
            stop_on_error: true,
            max_parallel: None,
            timeout_ms: None,
            save: None,
            actions: vec![action("a", "b"), action("b", "a")],
        };
        let dispatch = |_t: String, _a: Value| async move { Ok(json!(null)) };
        let report = tokio::time::timeout(Duration::from_secs(5), run_batch(&s, dispatch))
            .await
            .expect("cycle deadlocked the executor");
        assert!(!report.ok);
        assert_eq!(report.skipped, 2);
    }

    #[tokio::test]
    async fn continue_on_error_sequential_still_runs_rest_in_order() {
        // stopOnError:false in sequential mode must keep executing the
        // remaining actions in order — only explicit dependsOn edges skip.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let s = spec(json!({
            "stopOnError": false,
            "actions": [
                { "tool": "one" },
                { "tool": "fail" },
                { "tool": "three" }
            ]
        }));
        let report = run_batch(&s, recording_dispatch(calls.clone())).await;
        assert!(!report.ok);
        assert_eq!(report.succeeded, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 0);
        assert_eq!(*calls.lock().await, vec!["one", "fail", "three"]);
    }

    #[tokio::test]
    async fn soft_error_result_counts_as_failure() {
        // A tool "succeeding" with {"error": ...} (e.g. element not found)
        // must fail the action and trigger stopOnError for the rest.
        let s = spec(json!({
            "actions": [
                { "id": "click", "tool": "webview_interact" },
                { "tool": "read_logs" }
            ]
        }));
        let dispatch = |_t: String, _a: Value| async move {
            Ok(json!({ "error": "Element not found", "selector": "#missing" }))
        };
        let report = run_batch(&s, dispatch).await;
        assert!(!report.ok);
        assert_eq!(report.logs[0].status, "error");
        let msg = report.logs[0].error.as_deref().unwrap();
        assert!(msg.contains("Element not found"), "message: {msg}");
        assert!(msg.contains("#missing"), "detail lost: {msg}");
        assert_eq!(report.logs[1].status, "skipped");
    }

    #[test]
    fn unknown_spec_fields_are_rejected() {
        // Typos like "timeout" (instead of timeoutMs) must not be silently
        // ignored.
        let err = BatchSpec::parse(&json!({
            "actions": [ { "tool": "a", "timeout": 5000 } ]
        }))
        .unwrap_err();
        assert!(err.contains("timeout"), "unexpected error: {err}");
        assert!(BatchSpec::parse(&json!({
            "mod": "parallel",
            "actions": [ { "tool": "a" } ]
        }))
        .is_err());
    }

    #[tokio::test]
    async fn synthesized_log_id_avoids_explicit_collision() {
        let s = spec(json!({
            "mode": "parallel",
            "actions": [
                { "tool": "one" },
                { "id": "a0", "tool": "two" }
            ]
        }));
        let dispatch = |_t: String, _a: Value| async move { Ok(json!(null)) };
        let report = run_batch(&s, dispatch).await;
        let ids: Vec<&str> = report.logs.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids[1], "a0");
        assert_ne!(ids[0], "a0", "synthesized id collided with explicit id");
    }
}
