use rusqlite::{params, Connection};
use sieve_types::{PlannerCodexSession, RunId};
use std::collections::BTreeSet;
use std::path::PathBuf;
use uuid::Uuid;

const STATUS_OPEN: &str = "open";
const STATUS_EXECUTING: &str = "executing";
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredOpenLoop {
    pub(crate) loop_id: String,
    pub(crate) session_key: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) goal_summary: String,
    pub(crate) subject_names: Vec<String>,
    pub(crate) target_paths: Vec<String>,
    pub(crate) assistant_context: String,
    pub(crate) next_expected_user_act: String,
    pub(crate) ready_for_execution: bool,
    pub(crate) linked_codex_session_id: Option<String>,
    pub(crate) linked_codex_session_name: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct OpenLoopStore {
    path: PathBuf,
}

impl OpenLoopStore {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Result<Self, String> {
        let store = Self { path: path.into() };
        store.init()?;
        Ok(store)
    }

    pub(crate) fn upsert_loop(&self, loop_record: &StoredOpenLoop) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO open_loops (
                loop_id, session_key, kind, status, goal_summary, subject_names_json,
                target_paths_json, assistant_context, next_expected_user_act,
                ready_for_execution, linked_codex_session_id, linked_codex_session_name,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(loop_id) DO UPDATE SET
                status=excluded.status,
                goal_summary=excluded.goal_summary,
                subject_names_json=excluded.subject_names_json,
                target_paths_json=excluded.target_paths_json,
                assistant_context=excluded.assistant_context,
                next_expected_user_act=excluded.next_expected_user_act,
                ready_for_execution=excluded.ready_for_execution,
                linked_codex_session_id=excluded.linked_codex_session_id,
                linked_codex_session_name=excluded.linked_codex_session_name,
                updated_at_ms=excluded.updated_at_ms",
            params![
                loop_record.loop_id,
                loop_record.session_key,
                loop_record.kind,
                loop_record.status,
                loop_record.goal_summary,
                encode_string_vec(&loop_record.subject_names)?,
                encode_string_vec(&loop_record.target_paths)?,
                loop_record.assistant_context,
                loop_record.next_expected_user_act,
                loop_record.ready_for_execution as i64,
                loop_record.linked_codex_session_id,
                loop_record.linked_codex_session_name,
                loop_record.created_at_ms as i64,
                loop_record.updated_at_ms as i64,
            ],
        )
        .map_err(|err| format!("upsert open loop failed: {err}"))?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn planner_open_loops(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Result<Vec<StoredOpenLoop>, String> {
        self.load_loops(session_key, &[STATUS_OPEN], limit)
    }

    pub(crate) fn referenced_loop_for_status(
        &self,
        session_key: &str,
        message: &str,
    ) -> Result<Option<StoredOpenLoop>, String> {
        let loops = self.load_loops(session_key, &[STATUS_OPEN, STATUS_EXECUTING], 16)?;
        Ok(best_matching_loop(message, &loops, false))
    }

    pub(crate) fn active_loop_for_followup(
        &self,
        session_key: &str,
        message: &str,
    ) -> Result<Option<StoredOpenLoop>, String> {
        let loops = self.load_loops(session_key, &[STATUS_OPEN], 16)?;
        if let Some(loop_record) = best_matching_loop(message, &loops, false) {
            return Ok(Some(loop_record));
        }
        if looks_like_open_loop_confirmation(message) {
            return Ok(best_matching_loop(message, &loops, true).or_else(|| {
                loops
                    .iter()
                    .find(|loop_record| loop_record.ready_for_execution)
                    .cloned()
            }));
        }
        if looks_like_short_open_loop_followup(message) && loops.len() == 1 {
            return Ok(loops.first().cloned());
        }
        Ok(None)
    }

    pub(crate) fn mark_executing(
        &self,
        loop_id: &str,
        linked_codex_session_id: Option<&str>,
        linked_codex_session_name: Option<&str>,
        updated_at_ms: u64,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "UPDATE open_loops
             SET status = ?2,
                 ready_for_execution = 0,
                 linked_codex_session_id = COALESCE(?3, linked_codex_session_id),
                 linked_codex_session_name = COALESCE(?4, linked_codex_session_name),
                 updated_at_ms = ?5
             WHERE loop_id = ?1",
            params![
                loop_id,
                STATUS_EXECUTING,
                linked_codex_session_id,
                linked_codex_session_name,
                updated_at_ms as i64,
            ],
        )
        .map_err(|err| format!("mark open loop executing failed: {err}"))?;
        Ok(())
    }

    fn load_loops(
        &self,
        session_key: &str,
        statuses: &[&str],
        limit: usize,
    ) -> Result<Vec<StoredOpenLoop>, String> {
        let status_list = statuses
            .iter()
            .map(|value| format!("'{}'", value.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT loop_id, session_key, kind, status, goal_summary, subject_names_json,
                    target_paths_json, assistant_context, next_expected_user_act,
                    ready_for_execution, linked_codex_session_id, linked_codex_session_name,
                    created_at_ms, updated_at_ms
             FROM open_loops
             WHERE session_key = ?1
               AND status IN ({status_list})
             ORDER BY updated_at_ms DESC
             LIMIT ?2"
        );
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| format!("prepare open loop query failed: {err}"))?;
        let rows = stmt
            .query_map(params![session_key, limit as i64], |row| {
                Ok(StoredOpenLoop {
                    loop_id: row.get(0)?,
                    session_key: row.get(1)?,
                    kind: row.get(2)?,
                    status: row.get(3)?,
                    goal_summary: row.get(4)?,
                    subject_names: decode_string_vec(&row.get::<_, String>(5)?)?,
                    target_paths: decode_string_vec(&row.get::<_, String>(6)?)?,
                    assistant_context: row.get(7)?,
                    next_expected_user_act: row.get(8)?,
                    ready_for_execution: row.get::<_, i64>(9)? != 0,
                    linked_codex_session_id: row.get(10)?,
                    linked_codex_session_name: row.get(11)?,
                    created_at_ms: row.get::<_, i64>(12)? as u64,
                    updated_at_ms: row.get::<_, i64>(13)? as u64,
                })
            })
            .map_err(|err| format!("query open loops failed: {err}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("read open loop failed: {err}"))?);
        }
        Ok(out)
    }

    fn init(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("create open loop store dir failed: {err}"))?;
        }
        let conn = self.open()?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS open_loops (
                loop_id TEXT PRIMARY KEY,
                session_key TEXT NOT NULL,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                goal_summary TEXT NOT NULL,
                subject_names_json TEXT NOT NULL,
                target_paths_json TEXT NOT NULL,
                assistant_context TEXT NOT NULL,
                next_expected_user_act TEXT NOT NULL,
                ready_for_execution INTEGER NOT NULL,
                linked_codex_session_id TEXT,
                linked_codex_session_name TEXT,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS open_loops_session_status_updated_idx
                 ON open_loops(session_key, status, updated_at_ms DESC);",
        )
        .map_err(|err| format!("initialize open loop store failed: {err}"))?;
        Ok(())
    }

    fn open(&self) -> Result<Connection, String> {
        Connection::open(&self.path).map_err(|err| {
            format!(
                "open working-state sqlite store {} failed: {err}",
                self.path.display()
            )
        })
    }
}

pub(crate) fn build_open_loop_from_preference_turn(
    session_key: &str,
    _run_id: &RunId,
    user_message: &str,
    assistant_context: &str,
    previous: Option<&StoredOpenLoop>,
    now_ms: u64,
) -> StoredOpenLoop {
    let target_paths = merge_string_lists(
        previous.map(|value| value.target_paths.as_slice()),
        &extract_path_tokens(user_message, assistant_context),
    );
    let subject_names = merge_string_lists(
        previous.map(|value| value.subject_names.as_slice()),
        &infer_subject_names(user_message, &target_paths),
    );
    StoredOpenLoop {
        loop_id: previous
            .map(|value| value.loop_id.clone())
            .unwrap_or_else(|| format!("open-loop-{}", Uuid::new_v4())),
        session_key: session_key.to_string(),
        kind: "proposal".to_string(),
        status: STATUS_OPEN.to_string(),
        goal_summary: first_non_empty_line(user_message),
        subject_names,
        target_paths,
        assistant_context: assistant_context.trim().to_string(),
        next_expected_user_act: if indicates_ready_to_proceed(assistant_context) {
            "confirm_or_answer".to_string()
        } else {
            "answer_questions".to_string()
        },
        ready_for_execution: indicates_ready_to_proceed(assistant_context),
        linked_codex_session_id: previous.and_then(|value| value.linked_codex_session_id.clone()),
        linked_codex_session_name: previous
            .and_then(|value| value.linked_codex_session_name.clone()),
        created_at_ms: previous.map(|value| value.created_at_ms).unwrap_or(now_ms),
        updated_at_ms: now_ms,
    }
}

pub(crate) fn format_open_loop_context_message(loop_record: &StoredOpenLoop) -> String {
    let subject_names = if loop_record.subject_names.is_empty() {
        "[]".to_string()
    } else {
        format!("{:?}", loop_record.subject_names)
    };
    let target_paths = if loop_record.target_paths.is_empty() {
        "[]".to_string()
    } else {
        format!("{:?}", loop_record.target_paths)
    };
    format!(
        "TRUSTED_OPEN_LOOP_CONTEXT:\n- loop_id: {}\n- kind: {}\n- status: {}\n- goal_summary: {}\n- subject_names: {}\n- target_paths: {}\n- next_expected_user_act: {}\n- ready_for_execution: {}\n- Preserve this target unless the user explicitly redirects.\n- Do not resume an unrelated saved Codex session just because one exists.\n- Prior assistant context:\n{}",
        loop_record.loop_id,
        loop_record.kind,
        loop_record.status,
        loop_record.goal_summary,
        subject_names,
        target_paths,
        loop_record.next_expected_user_act,
        loop_record.ready_for_execution,
        loop_record.assistant_context
    )
}

pub(crate) fn format_open_loop_status_reply(
    loop_record: &StoredOpenLoop,
    linked_session: Option<&PlannerCodexSession>,
) -> String {
    if let Some(session) = linked_session {
        return match session.status.as_str() {
            "completed" => format!(
                "It’s done, not ongoing. The saved session `{}` is marked `completed`, last updated at `{}`, and its summary says {}.",
                session.session_name,
                session.updated_at_utc,
                session
                    .last_result_summary
                    .as_deref()
                    .unwrap_or("the work completed")
            ),
            "failed" => format!(
                "It failed. The saved session `{}` is marked `failed`, last updated at `{}`, and the latest summary says {}.",
                session.session_name,
                session.updated_at_utc,
                session
                    .last_result_summary
                    .as_deref()
                    .unwrap_or("the last Codex turn failed")
            ),
            "needs_followup" => format!(
                "It is not done yet. The saved session `{}` needs follow-up, last updated at `{}`, and the latest summary says {}.",
                session.session_name,
                session.updated_at_utc,
                session
                    .last_result_summary
                    .as_deref()
                    .unwrap_or("more work remains")
            ),
            "waiting_approval" => format!(
                "It’s waiting on approval right now. The saved session `{}` last updated at `{}`, and the latest status says {}.",
                session.session_name,
                session.updated_at_utc,
                session
                    .last_result_summary
                    .as_deref()
                    .unwrap_or("Codex requested approval before it can continue")
            ),
            _ => format!(
                "It’s still running. The saved session `{}` is marked `{}`, last updated at `{}`, and the task summary is {}.",
                session.session_name,
                session.status,
                session.updated_at_utc,
                session.task_summary
            ),
        };
    }
    let subject = open_loop_display_name(loop_record);
    match loop_record.status.as_str() {
        STATUS_OPEN if loop_record.ready_for_execution => format!(
            "I haven’t started `{subject}` yet. I have a proposed plan ready, and I’m waiting for your go-ahead before I start execution."
        ),
        STATUS_OPEN => format!(
            "I haven’t started `{subject}` yet. I’m still waiting on your answers or decisions before I start execution."
        ),
        STATUS_EXECUTING => format!(
            "I’ve already started `{subject}`, but I don’t have a linked saved Codex status update yet."
        ),
        _ => format!("`{subject}` is no longer an active open loop."),
    }
}

pub(crate) fn linked_session_for_loop<'a>(
    loop_record: &StoredOpenLoop,
    sessions: &'a [PlannerCodexSession],
) -> Option<&'a PlannerCodexSession> {
    let linked_id = loop_record.linked_codex_session_id.as_deref()?;
    sessions
        .iter()
        .find(|session| session.session_id == linked_id)
}

pub(crate) fn looks_like_open_loop_confirmation(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "go ahead",
        "use codex",
        "proceed",
        "use defaults",
        "do it",
        "start it",
        "sounds good",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_short_open_loop_followup(message: &str) -> bool {
    let trimmed = message.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= 120 && !trimmed.contains('\n')
}

fn best_matching_loop(
    message: &str,
    loops: &[StoredOpenLoop],
    ready_only: bool,
) -> Option<StoredOpenLoop> {
    let lower = message.to_ascii_lowercase();
    loops
        .iter()
        .filter(|loop_record| !ready_only || loop_record.ready_for_execution)
        .filter_map(|loop_record| {
            let mut score = 0usize;
            for path in &loop_record.target_paths {
                if lower.contains(&path.to_ascii_lowercase()) {
                    score = score.max(100);
                }
                if let Some(name) = basename(path) {
                    if lower.contains(&name.to_ascii_lowercase()) {
                        score = score.max(80);
                    }
                }
            }
            for subject in &loop_record.subject_names {
                if lower.contains(&subject.to_ascii_lowercase()) {
                    score = score.max(60);
                }
            }
            (score > 0).then_some((score, loop_record.clone()))
        })
        .max_by_key(|(score, loop_record)| (*score, loop_record.updated_at_ms))
        .map(|(_, loop_record)| loop_record)
}

fn open_loop_display_name(loop_record: &StoredOpenLoop) -> String {
    loop_record
        .subject_names
        .first()
        .cloned()
        .or_else(|| {
            loop_record
                .target_paths
                .iter()
                .find_map(|path| basename(path).map(ToString::to_string))
        })
        .unwrap_or_else(|| loop_record.goal_summary.clone())
}

fn first_non_empty_line(input: &str) -> String {
    input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("pending task")
        .to_string()
}

fn indicates_ready_to_proceed(context: &str) -> bool {
    let lower = context.to_ascii_lowercase();
    lower.contains("if you don’t answer, i’ll proceed")
        || lower.contains("if you don't answer, i'll proceed")
        || lower.contains("if you want me to proceed")
        || lower.contains("once you confirm")
        || lower.contains("proceed with these defaults")
}

fn extract_path_tokens(user_message: &str, assistant_context: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for token in user_message
        .split_whitespace()
        .chain(assistant_context.split_whitespace())
    {
        let normalized = token
            .trim_matches(|ch: char| {
                matches!(
                    ch,
                    '`' | '"' | '\'' | ',' | '.' | ':' | ';' | '(' | ')' | '[' | ']'
                )
            })
            .trim();
        if normalized.starts_with("~/") || normalized.starts_with('/') {
            if seen.insert(normalized.to_string()) {
                out.push(normalized.to_string());
            }
        }
    }
    out
}

fn infer_subject_names(user_message: &str, target_paths: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for path in target_paths {
        if let Some(name) = basename(path) {
            if seen.insert(name.to_string()) {
                out.push(name.to_string());
            }
        }
    }
    for token in user_message
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
        .map(str::trim)
        .filter(|token| token.len() >= 4)
    {
        let lower = token.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "create"
                | "project"
                | "going"
                | "golang"
                | "cobra"
                | "learning"
                | "science"
                | "aware"
                | "tool"
                | "storage"
                | "agent"
                | "framework"
                | "specifically"
                | "subcommands"
                | "allow"
                | "things"
                | "break"
                | "down"
                | "texts"
                | "resources"
                | "conceptual"
                | "dependency"
                | "graphs"
                | "clarifying"
                | "questions"
                | "then"
                | "proceed"
        ) {
            continue;
        }
        if seen.insert(lower.clone()) {
            out.push(lower);
        }
        if out.len() >= 6 {
            break;
        }
    }
    out
}

fn merge_string_lists(existing: Option<&[String]>, additional: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    if let Some(existing) = existing {
        for value in existing {
            if seen.insert(value.clone()) {
                out.push(value.clone());
            }
        }
    }
    for value in additional {
        if seen.insert(value.clone()) {
            out.push(value.clone());
        }
    }
    out
}

fn encode_string_vec(values: &[String]) -> Result<String, String> {
    serde_json::to_string(values).map_err(|err| format!("encode open loop strings failed: {err}"))
}

fn decode_string_vec(raw: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn basename(path: &str) -> Option<&str> {
    path.rsplit('/').next().filter(|value| !value.is_empty())
}
