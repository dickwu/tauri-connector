use super::{CaptureOptions, error, now_ms, redact};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_SESSIONS: usize = 8;
const MAX_INVOCATIONS: usize = 10_000;
#[derive(Clone)]
struct Session {
    domain: u64,
    window: String,
    options: CaptureOptions,
    sources: Vec<String>,
    start: u64,
    end: Option<u64>,
    stopped_at: Option<Instant>,
    deadline: Instant,
    expired: bool,
    dropped: u64,
    applied: bool,
    acknowledged_at: Option<u64>,
}
#[derive(Clone)]
struct Source {
    window: String,
    context: Value,
    dropped: u64,
    interrupted: bool,
    retired_sequence: u64,
    hook_source: String,
}
struct StoredEvent {
    seq: u64,
    source: String,
    bytes: usize,
    value: Value,
}
struct Invocation {
    source: String,
    id: String,
    command: String,
    started: Option<u64>,
    terminal: Option<u64>,
    interrupted: bool,
    observed_at: u64,
    seq: u64,
}
pub(super) struct Store {
    namespace: String,
    sessions: HashMap<String, Session>,
    sources: HashMap<String, Source>,
    events: VecDeque<StoredEvent>,
    invocations: HashMap<String, Invocation>,
    sequence: u64,
    seen: HashMap<String, u64>,
    bytes: usize,
    pub(super) max_events: usize,
    pub(super) reservation: Option<crate::workflow::budget::Reservation>,
    pub(super) session_lifetime: Duration,
    pub(super) retention: Duration,
    pending_cleanup: HashMap<String, Value>,
    pub(super) preview_commands: Vec<String>,
    pub(super) preview_paths: Vec<String>,
}
impl Default for Store {
    fn default() -> Self {
        let setting = |name| {
            std::env::var(name)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .take(50)
                .map(str::to_owned)
                .collect()
        };
        Self {
            namespace: uuid::Uuid::new_v4().to_string(),
            sessions: HashMap::new(),
            sources: HashMap::new(),
            events: VecDeque::new(),
            invocations: HashMap::new(),
            sequence: 0,
            seen: HashMap::new(),
            bytes: 0,
            max_events: 10_000,
            reservation: None,
            session_lifetime: Duration::from_millis(
                std::env::var("TAURI_CONNECTOR_CAPTURE_MAX_DURATION_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(600_000)
                    .clamp(1, 600_000),
            ),
            retention: Duration::from_secs(900),
            pending_cleanup: HashMap::new(),
            preview_commands: setting("TAURI_CONNECTOR_CAPTURE_PREVIEW_COMMANDS"),
            preview_paths: setting("TAURI_CONNECTOR_CAPTURE_PREVIEW_PATHS"),
        }
    }
}
impl Store {
    pub(super) fn release_if_empty(&mut self) {
        if self.sessions.is_empty() {
            self.events.clear();
            self.invocations.clear();
            self.seen.clear();
            self.sources.clear();
            self.bytes = 0;
            self.reservation = None;
        }
    }
    pub(super) fn note_hook(&mut self, id: &str, reply: &Value) {
        if let Some(source) = self
            .sessions
            .get(id)
            .and_then(|s| s.sources.last())
            .cloned()
        {
            self.note_source_hook(&source, reply);
        }
    }
    pub(super) fn note_source_hook(&mut self, source: &str, reply: &Value) {
        if let Some(kind) = reply["coverage"]["frontendInvoke"].as_str().filter(|kind| {
            matches!(
                *kind,
                "frontend_invoke_wrapper" | "frontend_tauri_transport_callbacks"
            )
        }) && let Some(s) = self.sources.get_mut(source)
        {
            s.hook_source = kind.into();
        }
    }
    pub(super) fn active_domain(&self, source: &str) -> Option<u64> {
        self.sessions
            .values()
            .find(|s| {
                s.end.is_none()
                    && s.deadline > Instant::now()
                    && s.sources.iter().any(|id| id == source)
            })
            .map(|s| s.domain)
    }

    pub(super) fn accepts_source(&self, window: &str, source: &str) -> bool {
        self.sources
            .get(source)
            .is_some_and(|s| s.window == window && !s.interrupted)
            && self.sessions.values().any(|s| {
                s.end.is_none()
                    && s.deadline > Instant::now()
                    && s.sources.iter().any(|id| id == source)
            })
    }

    pub(super) fn has_follow_session(&self, window: &str) -> bool {
        self.sessions.values().any(|s| {
            s.window == window
                && s.end.is_none()
                && s.deadline > Instant::now()
                && s.options.follow_pages
        })
    }
    fn queue_cleanup(&mut self, source: &str) {
        if let Some(s) = self.sources.get(source) {
            self.pending_cleanup.insert(
                s.window.clone(),
                json!({"windowId":s.window,"sourceId":source,"context":s.context}),
            );
        }
    }
    pub(super) fn take_cleanup(&mut self) -> Vec<Value> {
        let now = Instant::now();
        std::mem::take(&mut self.pending_cleanup)
            .into_values()
            .map(|mut job| {
                job["sessionIds"] = json!(
                    self.sessions
                        .iter()
                        .filter(|(_, s)| s.end.is_none()
                            && s.deadline > now
                            && s.sources
                                .last()
                                .is_some_and(|id| job["sourceId"] == id.as_str()))
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>()
                );
                job
            })
            .collect()
    }
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        if !self.pending_cleanup.is_empty() {
            return Some(Instant::now());
        }
        self.sessions
            .values()
            .map(|s| s.stopped_at.map_or(s.deadline, |at| at + self.retention))
            .min()
    }
    pub(super) fn expire(&mut self) {
        self.expire_at(Instant::now());
    }
    pub(super) fn expire_at(&mut self, now: Instant) {
        let mut expired_sources = Vec::new();
        for session in self
            .sessions
            .values_mut()
            .filter(|s| s.end.is_none() && s.deadline <= now)
        {
            session.end = Some(self.sequence);
            session.stopped_at = Some(session.deadline);
            session.expired = true;
            session.applied = false;
            if let Some(source) = session.sources.last() {
                expired_sources.push(source.clone());
            }
        }
        for source in expired_sources {
            self.queue_cleanup(&source);
        }
        self.sessions.retain(|_, s| {
            s.stopped_at
                .is_none_or(|at| now.saturating_duration_since(at) < self.retention)
        });
        self.release_if_empty();
    }
    pub(super) fn acknowledge_source(&mut self, source: &str, applied: bool) {
        for s in self
            .sessions
            .values_mut()
            .filter(|s| s.end.is_none() && s.sources.last().is_some_and(|id| id == source))
        {
            let applied = applied && s.deadline > Instant::now();
            s.applied = applied;
            s.acknowledged_at = applied.then(now_ms);
        }
    }
    pub(super) fn source(&mut self, window: &str, context: Value) -> String {
        if let Some((id, _)) = self
            .sources
            .iter()
            .find(|(_, s)| s.window == window && s.context == context && !s.interrupted)
        {
            return id.clone();
        }
        // Sources with no retained session are not useful evidence and may be removed.
        self.sources
            .retain(|id, _| self.sessions.values().any(|s| s.sources.contains(id)));
        let id = uuid::Uuid::new_v4().to_string();
        self.sources.insert(
            id.clone(),
            Source {
                window: window.to_owned(),
                context,
                dropped: 0,
                interrupted: false,
                retired_sequence: 0,
                hook_source: "unacknowledged".into(),
            },
        );
        id
    }
    #[cfg(test)]
    pub(super) fn start(
        &mut self,
        domain: u64,
        window: &str,
        options: CaptureOptions,
        source: &str,
    ) -> Result<String, Value> {
        self.start_at(domain, window, options, source, Instant::now())
    }
    pub(super) fn start_at(
        &mut self,
        domain: u64,
        window: &str,
        options: CaptureOptions,
        source: &str,
        started_at: Instant,
    ) -> Result<String, Value> {
        let deadline = started_at + self.session_lifetime;
        if deadline <= Instant::now() {
            return Err(error(
                "capture_expired",
                "Capture lifetime elapsed during preparation",
            ));
        }
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(error(
                "resource_exhausted",
                "At most 8 retained capture sessions; stop sessions expire after 15 minutes",
            ));
        }
        if (options.result_policy == "preview" || options.argument_policy == "preview")
            && self.preview_commands.is_empty()
        {
            return Err(error(
                "preview_not_allowed",
                "The host must allow preview commands before preview capture",
            ));
        }
        let id = format!(
            "{}:capture:{}",
            crate::identity::app_instance_id(),
            uuid::Uuid::new_v4()
        );
        self.sessions.insert(
            id.clone(),
            Session {
                domain,
                window: window.into(),
                options,
                sources: vec![source.into()],
                start: self.sequence + 1,
                end: None,
                stopped_at: None,
                deadline,
                expired: false,
                dropped: 0,
                applied: false,
                acknowledged_at: None,
            },
        );
        Ok(id)
    }
    fn session(&self, id: &str, domain: u64) -> Result<&Session, Value> {
        self.sessions
            .get(id)
            .filter(|s| s.domain == domain)
            .ok_or_else(|| {
                error(
                    "capture_not_found",
                    "Capture session is unavailable in this authorization domain",
                )
            })
    }
    fn cursor(&self, id: &str, seq: u64) -> String {
        format!("{}.{}.{seq}", self.namespace, id)
    }
    pub(super) fn status(&self, id: &str, domain: u64) -> Result<Value, Value> {
        let s = self.session(id, domain)?;
        let context = s
            .sources
            .last()
            .and_then(|id| self.sources.get(id))
            .map(|s| s.context.clone());
        Ok(
            json!({"captureSessionId":id,"desired":s.end.is_none()&&s.deadline>Instant::now(),"applied":s.applied&&s.deadline>Instant::now(),"status":if s.expired{"expired"}else if s.end.is_some(){"stopped"}else{"active"},"remainingMs":s.deadline.saturating_duration_since(Instant::now()).as_millis() as u64,"context":context,"acknowledgedAt":s.acknowledged_at,"startSequence":s.start,"endSequence":s.end,"droppedEvents":s.dropped,"coverage":self.coverage(s)}),
        )
    }
    fn coverage(&self, s: &Session) -> Value {
        json!({"source":"frontend_invoke_observation","hookSources":s.sources.iter().filter_map(|id|self.sources.get(id).map(|s|s.hook_source.clone())).collect::<std::collections::BTreeSet<_>>(),"beforeStart":"unobserved","cachedBeforeInstall":"unobserved","rustTrace":"unobserved","businessPersistence":"unobserved","correlation":"none","followPages":s.options.follow_pages,"pageGaps":s.sources.iter().filter(|id|self.sources.get(*id).is_some_and(|v|v.interrupted)).count(),"previewPolicy":"host_command_and_string_path_allowlist","pendingMayBeIncomplete":s.dropped>0,"promiseIdentity":if s.sources.iter().any(|id|self.sources.get(id).is_some_and(|s|s.hook_source=="frontend_invoke_wrapper")){"derived_promise"}else if s.sources.iter().any(|id|self.sources.get(id).is_some_and(|s|s.hook_source=="frontend_tauri_transport_callbacks")){"preserved"}else{"unobserved"},"preTransportFailures":"unobserved_on_immutable_invoke"})
    }
    pub(super) fn acknowledge(&mut self, id: &str, applied: bool) {
        if let Some(s) = self.sessions.get_mut(id) {
            let applied = applied && s.end.is_none() && s.deadline > Instant::now();
            s.applied = applied;
            s.acknowledged_at = applied.then(now_ms);
        }
    }
    pub(super) fn remove(&mut self, id: &str) {
        self.sessions.remove(id);
        self.release_if_empty();
    }
    pub(super) fn stop(&mut self, id: &str, domain: u64) -> Result<Value, Value> {
        self.session(id, domain)?;
        let s = self.sessions.get_mut(id).unwrap();
        s.end.get_or_insert(self.sequence);
        s.stopped_at.get_or_insert(Instant::now());
        s.applied = false;
        self.status(id, domain)
    }
    pub(super) fn window(&self, id: &str, domain: u64) -> Result<String, Value> {
        Ok(self.session(id, domain)?.window.clone())
    }
    pub(super) fn config(&self, source: &str) -> Value {
        self.config_excluding(source, None)
    }
    pub(super) fn stop_config(&self, id: &str, domain: u64) -> Result<Option<Value>, Value> {
        let s = self.session(id, domain)?;
        if s.end.is_some() {
            return Ok(None);
        }
        Ok(s.sources.last().map(|source| {
            let mut result = self.config_excluding(source, Some(id));
            result["drain"] = json!(true);
            result
        }))
    }
    pub(super) fn note_gap(&mut self, id: &str, count: u64) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.dropped = s.dropped.saturating_add(count);
        }
    }
    fn config_excluding(&self, source: &str, excluded: Option<&str>) -> Value {
        let s = &self.sources[source];
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter(|(id, _)| Some(id.as_str()) != excluded)
            .filter(|(_, v)| {
                v.end.is_none()
                    && v.deadline > Instant::now()
                    && v.sources.last().is_some_and(|id| id == source)
            })
            .collect();
        json!({"windowId":s.window,"context":s.context,"sourceId":source,"enabled":!sessions.is_empty(),"sessions":sessions.iter().map(|(id,s)|json!({"captureSessionId":id,"remainingMs":s.deadline.saturating_duration_since(Instant::now()).as_millis() as u64,"resultPolicy":s.options.result_policy,"argumentPolicy":s.options.argument_policy})).collect::<Vec<_>>(),"resultPolicy":if sessions.iter().any(|(_,s)|s.options.result_policy=="preview"){"preview"}else{"metadata"},"argumentPolicy":if sessions.iter().any(|(_,s)|s.options.argument_policy=="preview"){"preview"}else{"metadata"},"allowedCommands":self.preview_commands,"allowedPaths":self.preview_paths})
    }
    pub(super) fn interrupt_window(&mut self, window: &str) {
        let ids: Vec<_> = self
            .sources
            .iter()
            .filter(|(_, s)| s.window == window && !s.interrupted)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.sources.get_mut(id).unwrap().interrupted = true;
        }
        for invocation in self.invocations.values_mut() {
            if ids.contains(&invocation.source) {
                invocation.interrupted = true;
            }
        }
        for s in self.sessions.values_mut().filter(|s| {
            s.window == window && s.end.is_none() && s.sources.iter().any(|id| ids.contains(id))
        }) {
            s.applied = false;
            s.acknowledged_at = None;
            s.dropped += 1;
            if !s.options.follow_pages {
                s.end = Some(self.sequence);
                s.stopped_at = Some(Instant::now());
            }
        }
    }
    pub(super) fn follow_source(&mut self, window: &str, source: &str) {
        for s in self.sessions.values_mut().filter(|s| {
            s.window == window
                && s.end.is_none()
                && s.deadline > Instant::now()
                && s.options.follow_pages
        }) {
            if !s.sources.iter().any(|id| id == source) {
                s.sources.push(source.into());
                if s.sources.len() > 64 {
                    s.sources.remove(0);
                    s.dropped += 1;
                }
            }
        }
    }
    pub(super) fn authorize_domains(&mut self, authorized: impl Fn(u64) -> bool) {
        let revoked: Vec<_> = self
            .sessions
            .values()
            .filter(|s| !authorized(s.domain))
            .filter_map(|s| s.sources.last().cloned())
            .collect();
        for source in revoked {
            self.queue_cleanup(&source);
        }
        self.sessions.retain(|_, s| authorized(s.domain));
        self.release_if_empty();
    }
    pub(super) fn ingest(&mut self, window: &str, payload: Value) {
        let Some(source_id) = payload
            .get("sourceId")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 64)
        else {
            return;
        };
        let Some(source) = self
            .sources
            .get(source_id)
            .filter(|s| s.window == window && !s.interrupted)
            .cloned()
        else {
            return;
        };
        if !self
            .sessions
            .values()
            .any(|s| s.end.is_none() && s.sources.iter().any(|id| id == source_id))
        {
            return;
        }
        if let Some(total) = payload.get("droppedEvents").and_then(Value::as_u64) {
            let delta = total.saturating_sub(source.dropped);
            self.sources.get_mut(source_id).unwrap().dropped = total.max(source.dropped);
            for s in self
                .sessions
                .values_mut()
                .filter(|s| s.sources.iter().any(|id| id == source_id))
            {
                s.dropped = s.dropped.saturating_add(delta);
            }
        }
        for input in payload
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(50)
        {
            if serde_json::to_vec(input).map_or(true, |s| s.len() > 16_384) {
                self.drop_source(source_id);
                continue;
            }
            let (Some(seq), Some(id), Some(phase), Some(command)) = (
                input["sourceSequence"].as_u64(),
                input["invocationId"].as_str(),
                input["phase"].as_str(),
                input["command"].as_str(),
            ) else {
                continue;
            };
            if id.len() > 128
                || command.len() > 256
                || !matches!(phase, "started" | "succeeded" | "failed")
                || command.starts_with("plugin:connector|")
            {
                continue;
            }
            if seq <= source.retired_sequence {
                continue;
            }
            let duplicate_key = format!("{source_id}:{seq}");
            if self.seen.contains_key(&duplicate_key) {
                continue;
            }
            let invocation_key = format!("{source_id}:{id}");
            if let Some(old) = self.invocations.get(&invocation_key)
                && ((phase == "started" && old.started.is_some())
                    || (phase != "started" && old.terminal.is_some()))
            {
                continue;
            }
            self.sequence += 1;
            let ingest = self.sequence;
            let preview_allowed = self.preview_commands.iter().any(|c| c == command);
            let event_source = if input["source"] == "frontend_tauri_transport_callbacks" {
                "frontend_tauri_transport_callbacks"
            } else {
                "frontend_invoke_wrapper"
            };
            let mut event = json!({"schemaVersion":2,"eventId":format!("{}:{ingest}",self.namespace),"invocationId":id,"sourceSequence":seq,"ingestSequence":ingest,"context":source.context,"phase":phase,"command":command,"wallTimeMs":input["wallTimeMs"].as_u64(),"source":event_source,"businessPersistence":"unobserved","correlation":{"kind":"none","actionId":null}});
            if let Some(duration) = input["durationMs"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
            {
                event["durationMs"] = json!(duration);
            }
            let preview_field = match phase {
                "started" => "argsPreview",
                "failed" => "errorPreview",
                _ => "resultPreview",
            };
            let capture_field = if phase == "started" {
                "argsCapture"
            } else {
                "resultCapture"
            };
            if preview_allowed && !input[preview_field].is_null() {
                event[preview_field] = redact::preview(&input[preview_field], &self.preview_paths);
                let supplied = input[capture_field]["status"]
                    .as_str()
                    .unwrap_or("redacted");
                event[capture_field] = json!({"status":if matches!(supplied,"captured"|"redacted"|"omitted"|"truncated"|"unserializable"){supplied}else{"redacted"},"truncated":input[capture_field]["truncated"].as_bool().unwrap_or(false),"hostRedaction":true});
            } else {
                event[capture_field] = json!({"status":"omitted","truncated":false});
            }
            let bytes = serde_json::to_vec(&event).map_or(16_385, |v| v.len()) + 256;
            self.bytes += bytes;
            self.events.push_back(StoredEvent {
                seq: ingest,
                source: source_id.into(),
                bytes,
                value: event,
            });
            self.seen.insert(duplicate_key, ingest);
            let item = self
                .invocations
                .entry(invocation_key)
                .or_insert_with(|| Invocation {
                    source: source_id.into(),
                    id: id.into(),
                    command: command.into(),
                    started: None,
                    terminal: None,
                    interrupted: false,
                    observed_at: now_ms(),
                    seq: ingest,
                });
            if phase == "started" {
                item.started = input["wallTimeMs"].as_u64();
            } else {
                item.terminal = Some(ingest);
            }
            self.trim();
        }
    }
    fn drop_source(&mut self, source: &str) {
        for s in self
            .sessions
            .values_mut()
            .filter(|s| s.sources.iter().any(|id| id == source))
        {
            s.dropped = s.dropped.saturating_add(1);
        }
    }
    fn trim(&mut self) {
        // Fixed caps bound all indexes too; charge conservative per-record overhead.
        while self.events.len() > self.max_events
            || self.bytes
                + self.invocations.len() * 1024
                + self.seen.len() * 256
                + self
                    .sessions
                    .values()
                    .map(|s| {
                        s.options
                            .commands
                            .iter()
                            .map(|c| c.len() + 64)
                            .sum::<usize>()
                            + 8192
                    })
                    .sum::<usize>()
                + self.sources.len() * 4096
                > MAX_BYTES
        {
            let Some(item) = self.events.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(item.bytes);
            self.drop_source(&item.source);
            if let Some(source) = self.sources.get_mut(&item.source) {
                source.retired_sequence = source
                    .retired_sequence
                    .max(item.value["sourceSequence"].as_u64().unwrap_or(0));
            }
        }
        let oldest = self.events.front().map_or(self.sequence + 1, |e| e.seq);
        self.seen.retain(|_, seq| *seq >= oldest);
        while self.invocations.len() > MAX_INVOCATIONS {
            let Some(key) = self
                .invocations
                .iter()
                .min_by_key(|(_, v)| v.seq)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            let item = self.invocations.remove(&key).unwrap();
            self.drop_source(&item.source);
        }
        self.invocations
            .retain(|_, v| v.terminal.is_none() || v.seq >= oldest);
    }
    pub(super) fn query(&self, id: &str, domain: u64, args: &Value) -> Result<Value, Value> {
        let session = self.session(id, domain)?;
        let limit = args["limit"].as_u64().unwrap_or(100) as usize;
        let max_bytes = args["maxBytes"].as_u64().unwrap_or(32768) as usize;
        if !(1..=500).contains(&limit) || !(1024..=65536).contains(&max_bytes) {
            return Err(error(
                "invalid_argument",
                "Query limit or maxBytes is outside its bound",
            ));
        }
        let prefix = format!("{}.{}.", self.namespace, id);
        let requested = if let Some(cursor) = args.get("cursor") {
            cursor
                .as_str()
                .and_then(|v| v.strip_prefix(&prefix))
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| {
                    error(
                        "invalid_cursor",
                        "Cursor belongs to another capture session or process",
                    )
                })?
        } else {
            session.start.saturating_sub(1)
        };
        if requested > self.sequence {
            return Err(error("invalid_cursor", "Cursor exceeds available sequence"));
        }
        let oldest = self
            .events
            .front()
            .map_or(self.sequence + 1, |e| e.seq)
            .max(session.start);
        let expired = requested.saturating_add(1) < oldest;
        let mut next = requested.max(oldest.saturating_sub(1));
        let mut truncated = false;
        let mut result = json!({"status":if expired{"cursor_expired"}else{"ok"},"captureSessionId":id,"events":[],"pending":[],"nextCursor":self.cursor(id,next),"oldestAvailableCursor":self.cursor(id,oldest.saturating_sub(1)),"droppedEvents":session.dropped,"truncated":false,"coverage":self.coverage(session)});
        if serde_json::to_vec(&result).map_or(true, |v| v.len() + 128 > max_bytes) {
            result["coverage"] = json!({"source":"frontend_invoke_observation","status":"truncated","businessPersistence":"unobserved"});
            truncated = true;
        }
        if expired {
            result["gap"] = json!({"afterSequence":requested,"oldestAvailableSequence":oldest});
        }
        let mut used = serde_json::to_vec(&result).map_or(1024, |v| v.len()) + 128;
        let mut events = Vec::new();
        for item in self.events.iter().filter(|e| {
            e.seq > requested && e.seq >= session.start && e.seq <= session.end.unwrap_or(u64::MAX)
        }) {
            if !session.sources.contains(&item.source)
                || !session.options.matches(&item.value["command"])
                || args
                    .get("invocationId")
                    .is_some_and(|id| *id != item.value["invocationId"])
                || args.get("phase").is_some_and(|p| *p != item.value["phase"])
            {
                next = item.seq;
                continue;
            }
            let mut value = item.value.clone();
            value["captureSessionId"] = json!(id);
            let field = if value["phase"] == "started" {
                "argsPreview"
            } else if value["phase"] == "failed" {
                "errorPreview"
            } else {
                "resultPreview"
            };
            let policy = if value["phase"] == "started" {
                &session.options.argument_policy
            } else {
                &session.options.result_policy
            };
            if policy != "preview"
                || !self
                    .preview_commands
                    .iter()
                    .any(|command| value["command"] == command.as_str())
            {
                value.as_object_mut().unwrap().remove(field);
                let capture = if field == "argsPreview" {
                    "argsCapture"
                } else {
                    "resultCapture"
                };
                value[capture] = json!({"status":"omitted","truncated":false});
            }
            if let Some(preview) = value.get(field).cloned() {
                value[field] = redact::preview(&preview, &self.preview_paths);
            }
            let bytes = serde_json::to_vec(&value).unwrap().len();
            if events.len() >= limit || used + bytes > max_bytes {
                truncated = true;
                break;
            }
            used += bytes + 1;
            events.push(value);
            next = item.seq;
        }
        let mut pending = Vec::new();
        for p in self.invocations.values().filter(|p| {
            p.terminal
                .is_none_or(|end| end > session.end.unwrap_or(u64::MAX))
                && p.started.is_some()
                && p.seq >= session.start
                && p.seq <= session.end.unwrap_or(u64::MAX)
                && session.sources.contains(&p.source)
        }) {
            if args
                .get("invocationId")
                .is_some_and(|id| id.as_str() != Some(p.id.as_str()))
                || !session.options.matches(&json!(p.command))
            {
                continue;
            }
            let value = json!({"invocationId":p.id,"command":p.command,"windowId":session.window,"startedAt":p.started,"ageMs":now_ms().saturating_sub(p.observed_at),"observationStatus":if p.interrupted||session.end.is_some(){"observation_interrupted"}else{"pending"},"businessExecution":"unobserved"});
            let bytes = serde_json::to_vec(&value).unwrap().len();
            if pending.len() >= limit || used + bytes > max_bytes {
                truncated = true;
                break;
            }
            used += bytes + 1;
            pending.push(value);
        }
        result["events"] = json!(events);
        result["pending"] = json!(pending);
        result["nextCursor"] = json!(self.cursor(id, next));
        result["truncated"] = json!(truncated);
        Ok(result)
    }
}
#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
