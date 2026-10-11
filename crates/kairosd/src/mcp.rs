//! MCP server (Streamable HTTP, stateless). Every write goes through the
//! same engine and risk policy as the HTTP API, with author = claude.
//! **There is deliberately no approve tool.**

use crate::engine::{Engine, EngineError};
use crate::http::AppState;
use chrono::{DateTime, Duration, Utc};
use kairos_core::api;
use kairos_core::changeset::{Status, WriteResult};
use kairos_core::model::Author;
use kairos_core::task::TaskStatus;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

pub const INSTRUCTIONS: &str = "kairos manages Alec's calendar (iCloud is the source of truth) and his tasks.\n\
RISK POLICY (decided by the server, never by you): low-risk changes are applied immediately and can be undone \
(task blocks, tasks, deadlines, new one-off events that don't clash, re-keying events kairos created). High-risk \
changes are QUEUED FOR ALEC'S APPROVAL (status=pending): editing/moving/deleting events Alec created himself, \
anything recurring, anything starting within 2 hours, events that clash with fixed events, sharing to Family, \
deleting deadlines, dropping tasks, or more than 8 operations at once. Writes to Family (except share_to_family) and \
to read-only imports (Google feeds, holidays) are refused. You cannot approve changes: tell Alec what is pending and \
let him approve in the TUI, CLI or on his phone. Every write returns {changeset_id, risk, status, summary}.\n\
SCHEDULING: don't hand-place work blocks. Create/update tasks with an estimate and deadline and call plan; the \
deterministic scheduler places blocks in free time and reports shortfalls (\"2.5 h short before Fri 17:00\") with \
options — relay those honestly. Only create a specific block when Alec asks for a specific slot (create_event with \
task_id makes a pinned block).\n\
TIMES: ISO 8601 ('2026-10-16T17:00', with or without offset; Europe/London assumed) or plain dates for all-day; \
natural forms like 'fri 17:00' also work but ambiguous input is rejected. Keys: Inbox (default), Engineering, \
Careers, Sport, Travel, OQTS, Social.\n\
BULK: for more than one event, find them with search_events, then make ONE update_events / delete_events / \
move_events call with a plain-English summary — never loop the single-event tools (each would be a separate \
approval). For deletions or large/ambiguous batches, show Alec the plan (counts + a few before/after examples) \
first unless he already said exactly what to do. A batch is one approval and one undo.";

#[derive(Clone)]
pub struct KairosMcp {
    st: AppState,
    tool_router: ToolRouter<Self>,
}

fn ok_json<T: Serialize>(v: &T) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(v).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Engine errors become tool errors Claude can read (not protocol errors).
fn tool_err(e: EngineError) -> CallToolResult {
    let mut msg = e.to_string();
    if let EngineError::Conflict { diff, .. } = &e
        && !diff.is_empty()
    {
        msg.push_str(&format!("\n{diff}"));
    }
    CallToolResult::error(vec![ContentBlock::text(msg)])
}

impl KairosMcp {
    pub fn new(st: AppState) -> Self {
        KairosMcp {
            st,
            tool_router: Self::tool_router(),
        }
    }

    async fn run<T: Serialize + Send + 'static>(
        &self,
        f: impl FnOnce(&mut Engine) -> Result<T, EngineError> + Send + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let eng = self.st.engine.clone();
        let r = tokio::task::spawn_blocking(move || {
            let mut g = eng
                .lock()
                .map_err(|_| EngineError::BadRequest("engine lock poisoned".into()))?;
            f(&mut g)
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        match r {
            Ok(v) => ok_json(&v),
            Err(e) => Ok(tool_err(e)),
        }
    }
}

fn window(
    e: &Engine,
    from: Option<&str>,
    to: Option<&str>,
    default_days: i64,
) -> Result<(DateTime<Utc>, DateTime<Utc>), EngineError> {
    let now = e.now();
    let tz = e.tz();
    let p = |s: &str| -> Result<DateTime<Utc>, EngineError> {
        Ok(match api::parse_time(s, tz, now)? {
            api::TimeIn::At(t) => t,
            api::TimeIn::Date(d) => kairos_core::time::resolve_local(d.and_time(chrono::NaiveTime::MIN), tz),
        })
    };
    let from = from.map(p).transpose()?.unwrap_or(now);
    let to = to.map(p).transpose()?.unwrap_or(from + Duration::days(default_days));
    if to <= from || to - from > Duration::days(400) {
        return Err(EngineError::BadRequest(
            "bad window (to must be after from, max 400 days)".into(),
        ));
    }
    Ok((from, to))
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct ListEventsArgs {
    /// Window start (default now).
    pub from: Option<String>,
    /// Window end (default from + 7 days).
    pub to: Option<String>,
    /// Overlays to include: personal, family, background, imported (default personal+family).
    pub overlays: Option<Vec<String>>,
    /// Keys to include (default all).
    pub keys: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindFreeArgs {
    pub from: Option<String>,
    pub to: Option<String>,
    /// Minimum free minutes.
    pub duration_min: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateEventArgs {
    pub title: String,
    /// Start time, or a date for all-day.
    pub start: String,
    pub end: Option<String>,
    pub duration_min: Option<u32>,
    /// Key (default Personal).
    pub key: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    /// Alarm offsets, minutes before start.
    pub alarms: Option<Vec<i64>>,
    /// Make this a pinned work block for this task (only when Alec asked for this exact slot).
    pub task_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateEventArgs {
    pub uid: String,
    /// Source id if the UID exists in several calendars.
    pub source: Option<String>,
    pub title: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub duration_min: Option<u32>,
    /// Empty string clears it.
    pub location: Option<String>,
    /// Notes (DESCRIPTION). Empty string clears them.
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EventRefArgs {
    pub uid: String,
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveEventArgs {
    pub uid: String,
    pub source: Option<String>,
    /// Destination key.
    pub key: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SearchEventsArgs {
    /// Words that must all appear in the title (case-insensitive), e.g. "B10 lecture".
    pub query: Option<String>,
    /// Text that must appear in the location.
    pub location: Option<String>,
    /// Only these keys (e.g. ["Engineering"]). Default: all of Alec's keys.
    pub keys: Option<Vec<String>>,
    /// Also search the shared Family calendar (read-only except share). Default false.
    pub include_family: Option<bool>,
    /// Window start (default today).
    pub from: Option<String>,
    /// Window end (default from + 180 days).
    pub to: Option<String>,
    /// Max rows (default 300, max 1000).
    pub limit: Option<usize>,
    /// Skip this many rows (paging).
    pub offset: Option<usize>,
}

/// One row per event (a recurring series is one row with its occurrence count).
#[derive(Debug, Serialize)]
pub struct SearchRow {
    pub uid: String,
    pub source: String,
    pub key: Option<String>,
    pub title: String,
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// First occurrence in the window (local time, or a date for all-day).
    pub start: String,
    pub end: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub recurring: bool,
    #[serde(skip_serializing_if = "is_one")]
    pub occurrences: usize,
    /// "alec", "claude", "scheduler", or absent = made outside kairos (phone/Calendar.app/import).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

fn is_one(n: &usize) -> bool {
    *n == 1
}

#[derive(Debug, Serialize)]
pub struct SearchResp {
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<SearchRow>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EventRefArg {
    pub uid: String,
    /// Source id from search_events (optional; the UID usually suffices).
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EventEditArg {
    pub uid: String,
    pub source: Option<String>,
    pub title: Option<String>,
    /// New start (ISO local time, or a date for all-day).
    pub start: Option<String>,
    pub end: Option<String>,
    pub duration_min: Option<u32>,
    /// Empty string clears it.
    pub location: Option<String>,
    /// Notes (DESCRIPTION). Empty string clears them.
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateEventsArgs {
    /// One entry per event; only the fields given change.
    pub updates: Vec<EventEditArg>,
    /// What the batch does, in Alec's words, shown in his approval list
    /// (e.g. "Move room codes from titles into location").
    pub summary: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteEventsArgs {
    pub events: Vec<EventRefArg>,
    /// What the batch does (e.g. "Remove lectures for modules Alec doesn't take: B12, B18").
    pub summary: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveEventsArgs {
    pub events: Vec<EventRefArg>,
    /// Destination key.
    pub key: String,
    pub summary: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct ListTasksArgs {
    /// open (default) | done | dropped | all
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTaskArgs {
    pub title: String,
    pub key: String,
    /// Estimated minutes of work.
    pub estimate_min: u32,
    /// Deadline: date (all-day) or time. Creates a ⚑ deadline event too.
    pub due: Option<String>,
    pub earliest_start: Option<String>,
    /// 1 high … 3 low (default 2).
    pub priority: Option<u8>,
    pub splittable: Option<bool>,
    pub min_chunk_min: Option<u32>,
    pub max_chunk_min: Option<u32>,
    /// morning | afternoon | evening
    pub preferred_window: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateTaskArgs {
    /// Task id or unique title prefix.
    pub id: String,
    pub title: Option<String>,
    pub key: Option<String>,
    pub estimate_min: Option<u32>,
    pub remaining_min: Option<u32>,
    /// Empty string clears the deadline.
    pub due: Option<String>,
    pub priority: Option<u8>,
    pub splittable: Option<bool>,
    pub min_chunk_min: Option<u32>,
    pub max_chunk_min: Option<u32>,
    pub preferred_window: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompleteTaskArgs {
    pub id: String,
    /// Actual minutes spent (for estimate calibration).
    pub actual_min: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeadlineArgs {
    pub title: String,
    pub due: String,
    pub key: String,
    pub task_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct PlanArgs {
    /// Preview without writing.
    pub dry_run: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct ListChangesetsArgs {
    /// pending | applied | rejected | undone | failed
    pub status: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdArgs {
    pub id: String,
}

#[tool_router]
impl KairosMcp {
    #[tool(
        description = "List events in a window (all calendars, duplicates included with their source/overlay). Read-only."
    )]
    async fn list_events(&self, Parameters(a): Parameters<ListEventsArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            let (from, to) = window(e, a.from.as_deref(), a.to.as_deref(), 7)?;
            let overlays: Vec<String> = a.overlays.unwrap_or_else(|| vec!["personal".into(), "family".into()]);
            let keys = a.keys.map(|k| k.iter().map(|x| x.to_lowercase()).collect::<Vec<_>>());
            let evs: Vec<_> = e
                .events(from, to)?
                .into_iter()
                .filter(|i| overlays.iter().any(|o| o.eq_ignore_ascii_case(i.overlay.as_str())))
                .filter(|i| {
                    keys.as_ref()
                        .is_none_or(|k| i.colour_key().is_some_and(|ck| k.contains(&ck.to_lowercase())))
                })
                .collect();
            Ok(api::EventsResp { from, to, events: evs })
        })
        .await
    }

    #[tool(
        description = "Search Alec's events: title words, location, keys, date window (default today → +180 days). Returns compact rows (uid, source, key, title, location, start, end), one per event; recurring series appear once with an occurrence count. Use this before bulk edits. Read-only."
    )]
    async fn search_events(&self, Parameters(a): Parameters<SearchEventsArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            let today = e.now().with_timezone(&e.tz()).date_naive().to_string();
            let from = a.from.clone().unwrap_or(today);
            let (from, to) = window(e, Some(&from), a.to.as_deref(), 180)?;
            let tz = e.tz();
            let words: Vec<String> = a
                .query
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            let loc = a.location.as_deref().map(str::to_lowercase);
            let keys = a.keys.map(|k| k.iter().map(|x| x.to_lowercase()).collect::<Vec<_>>());
            let family = a.include_family.unwrap_or(false);
            let mut rows: Vec<SearchRow> = Vec::new();
            let mut seen: std::collections::HashMap<(String, String), usize> = Default::default();
            for i in e.events(from, to)? {
                let ok_overlay = match i.overlay {
                    kairos_core::config::Overlay::Personal => true,
                    kairos_core::config::Overlay::Family => family,
                    _ => false,
                };
                if !ok_overlay || i.source == "test" {
                    continue;
                }
                let t = i.title.to_lowercase();
                if !words.iter().all(|w| t.contains(w.as_str())) {
                    continue;
                }
                if let Some(l) = &loc
                    && !i
                        .location
                        .as_deref()
                        .is_some_and(|x| x.to_lowercase().contains(l.as_str()))
                {
                    continue;
                }
                if let Some(k) = &keys
                    && !i.colour_key().is_some_and(|ck| k.contains(&ck.to_lowercase()))
                {
                    continue;
                }
                let id = (i.source.clone(), i.uid.clone());
                if let Some(&n) = seen.get(&id) {
                    rows[n].occurrences += 1;
                    continue;
                }
                let fmt = |t: DateTime<Utc>| t.with_timezone(&tz).format("%Y-%m-%dT%H:%M").to_string();
                let (start, end) = match i.span {
                    kairos_core::model::Span::AllDay { start, end } => (start.to_string(), end.to_string()),
                    kairos_core::model::Span::Timed { start, end } => (fmt(start), fmt(end)),
                };
                seen.insert(id, rows.len());
                rows.push(SearchRow {
                    uid: i.uid.clone(),
                    source: i.source.clone(),
                    key: i.colour_key().map(String::from),
                    title: i.title.clone(),
                    location: i.location.clone(),
                    notes: i.description.clone(),
                    start,
                    end,
                    recurring: i.recurring,
                    occurrences: 1,
                    created_by: i.meta.created_by.map(|a| a.as_str().to_string()),
                });
            }
            let total = rows.len();
            let offset = a.offset.unwrap_or(0);
            let limit = a.limit.unwrap_or(300).min(1000);
            let rows = rows.into_iter().skip(offset).take(limit).collect();
            Ok(SearchResp { total, offset, rows })
        })
        .await
    }

    #[tool(
        description = "Edit many events in ONE changeset (one approval for Alec, one undo). Use for any change to more than one event, e.g. moving room codes from titles into the location field. Give each event's uid (from search_events) and only the fields that change. Changes to events Alec made himself, recurring series, or > 8 events are queued for his approval — tell him what is pending and how many events."
    )]
    async fn update_events(&self, Parameters(a): Parameters<UpdateEventsArgs>) -> Result<CallToolResult, ErrorData> {
        let updates: Vec<api::EventUpdate> = a
            .updates
            .into_iter()
            .map(|u| api::EventUpdate {
                uid: u.uid,
                source: u.source,
                change: api::UpdateEventReq {
                    title: u.title,
                    start: u.start,
                    end: u.end,
                    duration_min: u.duration_min,
                    location: u.location,
                    description: u.description,
                    blocking: None,
                },
            })
            .collect();
        self.run(move |e| {
            e.update_events(Author::Claude, &updates, Some(&a.summary))
                .map(|c| WriteResult::from(&c))
        })
        .await
    }

    #[tool(
        description = "Delete many events in ONE changeset (one approval, one undo restores all). E.g. remove lectures for modules Alec doesn't take. Deleting events Alec made himself is queued for his approval."
    )]
    async fn delete_events(&self, Parameters(a): Parameters<DeleteEventsArgs>) -> Result<CallToolResult, ErrorData> {
        let evs: Vec<(Option<String>, String)> = a.events.into_iter().map(|r| (r.source, r.uid)).collect();
        self.run(move |e| {
            e.delete_events(Author::Claude, &evs, Some(&a.summary))
                .map(|c| WriteResult::from(&c))
        })
        .await
    }

    #[tool(
        description = "Re-key many events (move them to another key's calendar) in ONE changeset (one approval, one undo)."
    )]
    async fn move_events(&self, Parameters(a): Parameters<MoveEventsArgs>) -> Result<CallToolResult, ErrorData> {
        let evs: Vec<(Option<String>, String)> = a.events.into_iter().map(|r| (r.source, r.uid)).collect();
        self.run(move |e| {
            e.move_events_labelled(Author::Claude, &evs, &a.key, Some(&a.summary))
                .map(|c| WriteResult::from(&c))
        })
        .await
    }

    #[tool(
        description = "Find free slots of at least duration_min inside the day window (fixed events and task blocks count as busy). Read-only."
    )]
    async fn find_free(&self, Parameters(a): Parameters<FindFreeArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            let (from, to) = window(e, a.from.as_deref(), a.to.as_deref(), 7)?;
            e.find_free(from, to, a.duration_min.max(5))
        })
        .await
    }

    #[tool(
        description = "Create a one-off event in a key calendar. Low risk (applied) if it doesn't clash with a fixed event and starts >2 h from now; otherwise queued for Alec's approval."
    )]
    async fn create_event(&self, Parameters(a): Parameters<CreateEventArgs>) -> Result<CallToolResult, ErrorData> {
        let req = api::CreateEventReq {
            title: a.title,
            start: a.start,
            end: a.end,
            duration_min: a.duration_min,
            key: a.key,
            location: a.location,
            description: a.description,
            alarms: a.alarms,
            task_id: a.task_id,
        };
        self.run(move |e| e.create_event(Author::Claude, &req).map(|c| WriteResult::from(&c)))
            .await
    }

    #[tool(
        description = "Change an event's title/time/location/notes. Events Alec created himself (phone/Calendar.app), recurring events and imminent events are queued for his approval."
    )]
    async fn update_event(&self, Parameters(a): Parameters<UpdateEventArgs>) -> Result<CallToolResult, ErrorData> {
        let req = api::UpdateEventReq {
            title: a.title,
            start: a.start,
            end: a.end,
            duration_min: a.duration_min,
            location: a.location,
            description: a.description,
            blocking: None,
        };
        self.run(move |e| {
            e.update_event(Author::Claude, a.source.as_deref(), &a.uid, &req)?
                .map(|c| WriteResult::from(&c))
                .ok_or_else(|| EngineError::BadRequest("nothing to change".into()))
        })
        .await
    }

    #[tool(description = "Delete an event. Deleting Alec's own events or deadlines needs his approval.")]
    async fn delete_event(&self, Parameters(a): Parameters<EventRefArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            e.delete_event(Author::Claude, a.source.as_deref(), &a.uid)
                .map(|c| WriteResult::from(&c))
        })
        .await
    }

    #[tool(
        description = "Re-key an event: move it to another key's calendar (keeps its UID). Low risk only for events kairos created."
    )]
    async fn move_event(&self, Parameters(a): Parameters<MoveEventArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            e.move_event(Author::Claude, a.source.as_deref(), &a.uid, &a.key)
                .map(|c| WriteResult::from(&c))
        })
        .await
    }

    #[tool(
        description = "Move an event from a personal calendar into the shared Family calendar. Always queued for Alec's approval."
    )]
    async fn share_to_family(&self, Parameters(a): Parameters<EventRefArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            e.share_to_family(Author::Claude, a.source.as_deref(), &a.uid)
                .map(|c| WriteResult::from(&c))
        })
        .await
    }

    #[tool(description = "List tasks (default: open), ordered by deadline.")]
    async fn list_tasks(&self, Parameters(a): Parameters<ListTasksArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            let st = match a.status.as_deref() {
                None | Some("open") => Some(TaskStatus::Open),
                Some("all") => None,
                Some(s) => {
                    Some(TaskStatus::parse(s).ok_or_else(|| EngineError::BadRequest(format!("bad status {s:?}")))?)
                }
            };
            Ok(e.db.tasks(st)?)
        })
        .await
    }

    #[tool(
        description = "Create a task with an estimate (and optional deadline, which also creates a ⚑ deadline event). Low risk. Then runs the scheduler and returns the plan, including any shortfall."
    )]
    async fn create_task(&self, Parameters(a): Parameters<CreateTaskArgs>) -> Result<CallToolResult, ErrorData> {
        let req = api::CreateTaskReq {
            title: a.title,
            key: a.key,
            estimate_min: a.estimate_min,
            due: a.due,
            earliest_start: a.earliest_start,
            priority: a.priority,
            splittable: a.splittable,
            min_chunk_min: a.min_chunk_min,
            max_chunk_min: a.max_chunk_min,
            preferred_window: a.preferred_window,
            notes: a.notes,
            create_deadline: None,
            plan: Some(true),
        };
        self.run(move |e| e.create_task(Author::Claude, &req)).await
    }

    #[tool(
        description = "Update a task (estimate, remaining, deadline, priority, chunking…). Low risk; replans afterwards."
    )]
    async fn update_task(&self, Parameters(a): Parameters<UpdateTaskArgs>) -> Result<CallToolResult, ErrorData> {
        let req = api::UpdateTaskReq {
            title: a.title,
            key: a.key,
            estimate_min: a.estimate_min,
            remaining_min: a.remaining_min,
            due: a.due,
            priority: a.priority,
            splittable: a.splittable,
            min_chunk_min: a.min_chunk_min,
            max_chunk_min: a.max_chunk_min,
            preferred_window: a.preferred_window,
            notes: a.notes,
            plan: Some(true),
        };
        self.run(move |e| e.update_task(Author::Claude, &a.id, &req)).await
    }

    #[tool(description = "Mark a task done (logs estimate vs actual); its future blocks are removed by the replan.")]
    async fn complete_task(&self, Parameters(a): Parameters<CompleteTaskArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| e.complete_task(Author::Claude, &a.id, a.actual_min))
            .await
    }

    #[tool(
        description = "Create a ⚑ deadline event (all-day for a date, zero-length with alarms for a time). Low risk."
    )]
    async fn create_deadline(&self, Parameters(a): Parameters<DeadlineArgs>) -> Result<CallToolResult, ErrorData> {
        let req = api::DeadlineReq {
            title: a.title,
            due: a.due,
            key: a.key,
            task_id: a.task_id,
        };
        self.run(move |e| e.create_deadline(Author::Claude, &req).map(|c| WriteResult::from(&c)))
            .await
    }

    #[tool(
        description = "Run the scheduler over open tasks. Keeps existing blocks where possible; reports blocks moved and per-task shortfalls with options. dry_run previews without writing."
    )]
    async fn plan(&self, Parameters(a): Parameters<PlanArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| e.plan(a.dry_run.unwrap_or(false), Author::Scheduler))
            .await
    }

    #[tool(description = "List changesets (newest first), e.g. status=pending for what's awaiting Alec's approval.")]
    async fn list_changesets(
        &self,
        Parameters(a): Parameters<ListChangesetsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            let st = a
                .status
                .as_deref()
                .map(|s| Status::parse(s).ok_or_else(|| EngineError::BadRequest(format!("bad status {s:?}"))))
                .transpose()?;
            let v: Vec<_> =
                e.db.cs_list(st, a.limit.unwrap_or(20).min(200))?
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id, "author": c.author, "created_at": c.created_at, "risk": c.risk,
                            "status": c.status, "summary": c.summary, "reasons": c.reasons, "error": c.error,
                            "ops": c.ops.iter().map(|o| o.describe()).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
            Ok(v)
        })
        .await
    }

    #[tool(description = "Full details of one changeset, including pre/post images.")]
    async fn get_changeset(&self, Parameters(a): Parameters<IdArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| {
            e.db.cs_get(&a.id)?
                .ok_or_else(|| EngineError::NotFound(format!("changeset {}", a.id)))
        })
        .await
    }

    #[tool(
        description = "Undo an applied changeset (restores pre-images). Refuses with a diff if the event was edited elsewhere since."
    )]
    async fn undo(&self, Parameters(a): Parameters<IdArgs>) -> Result<CallToolResult, ErrorData> {
        self.run(move |e| e.undo(&a.id).map(|c| WriteResult::from(&c))).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KairosMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("kairos", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// The `/mcp` route: stateless Streamable HTTP, JSON responses.
pub fn router(st: AppState, allowed_hosts: Vec<String>) -> axum::Router<AppState> {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
    let cfg = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None);
    let cfg = if allowed_hosts.is_empty() {
        cfg.disable_allowed_hosts()
    } else {
        cfg.with_allowed_hosts(allowed_hosts)
    };
    let svc: StreamableHttpService<KairosMcp, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(KairosMcp::new(st.clone())), Default::default(), cfg);
    axum::Router::new().nest_service("/mcp", svc)
}
