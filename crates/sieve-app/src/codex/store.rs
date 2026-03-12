use chrono::{SecondsFormat, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use sieve_types::{CodexSandboxMode, PlannerCodexSession};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredCodexSession {
    pub(crate) session_id: String,
    pub(crate) thread_id: String,
    pub(crate) session_name: String,
    pub(crate) cwd: String,
    pub(crate) sandbox: CodexSandboxMode,
    pub(crate) task_summary: String,
    pub(crate) last_result_summary: Option<String>,
    pub(crate) status: String,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexSessionStore {
    path: PathBuf,
}

impl CodexSessionStore {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Result<Self, String> {
        let store = Self { path: path.into() };
        store.init()?;
        Ok(store)
    }

    pub(crate) fn existing_session_names(&self) -> Result<BTreeSet<String>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare("SELECT session_name FROM codex_sessions ORDER BY session_name ASC")
            .map_err(|err| format!("prepare existing session names failed: {err}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| format!("query existing session names failed: {err}"))?;
        let mut out = BTreeSet::new();
        for row in rows {
            out.insert(row.map_err(|err| format!("read existing session name failed: {err}"))?);
        }
        Ok(out)
    }

    pub(crate) fn upsert_session(&self, session: &StoredCodexSession) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO codex_sessions (
                session_id, thread_id, session_name, cwd, sandbox, task_summary,
                last_result_summary, status, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(session_id) DO UPDATE SET
                thread_id=excluded.thread_id,
                session_name=excluded.session_name,
                cwd=excluded.cwd,
                sandbox=excluded.sandbox,
                task_summary=excluded.task_summary,
                last_result_summary=excluded.last_result_summary,
                status=excluded.status,
                updated_at_ms=excluded.updated_at_ms",
            params![
                session.session_id,
                session.thread_id,
                session.session_name,
                session.cwd,
                session.sandbox.as_str(),
                session.task_summary,
                session.last_result_summary,
                session.status,
                session.created_at_ms as i64,
                session.updated_at_ms as i64,
            ],
        )
        .map_err(|err| format!("upsert codex session failed: {err}"))?;
        Ok(())
    }

    pub(crate) fn session(&self, session_id: &str) -> Result<Option<StoredCodexSession>, String> {
        let conn = self.open()?;
        conn.query_row(
            "SELECT session_id, thread_id, session_name, cwd, sandbox, task_summary,
                    last_result_summary, status, created_at_ms, updated_at_ms
             FROM codex_sessions
             WHERE session_id = ?1",
            params![session_id],
            |row| {
                Ok(StoredCodexSession {
                    session_id: row.get(0)?,
                    thread_id: row.get(1)?,
                    session_name: row.get(2)?,
                    cwd: row.get(3)?,
                    sandbox: parse_sandbox(&row.get::<_, String>(4)?)?,
                    task_summary: row.get(5)?,
                    last_result_summary: row.get(6)?,
                    status: row.get(7)?,
                    created_at_ms: row.get::<_, i64>(8)? as u64,
                    updated_at_ms: row.get::<_, i64>(9)? as u64,
                })
            },
        )
        .optional()
        .map_err(|err| format!("load codex session failed: {err}"))
    }

    pub(crate) fn record_turn(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        instruction: &str,
        status: &str,
        summary: &str,
        user_visible: Option<&str>,
        created_at_ms: u64,
        completed_at_ms: u64,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO codex_turns (
                session_id, turn_id, instruction, status, summary, user_visible,
                created_at_ms, completed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id,
                turn_id,
                instruction,
                status,
                summary,
                user_visible,
                created_at_ms as i64,
                completed_at_ms as i64,
            ],
        )
        .map_err(|err| format!("insert codex turn failed: {err}"))?;
        Ok(())
    }

    pub(crate) fn record_event(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        event_type: &str,
        payload_json: &str,
        created_at_ms: u64,
    ) -> Result<(), String> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO codex_events (
                session_id, turn_id, event_type, payload_json, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                turn_id,
                event_type,
                payload_json,
                created_at_ms as i64,
            ],
        )
        .map_err(|err| format!("insert codex event failed: {err}"))?;
        Ok(())
    }

    pub(crate) fn planner_sessions(
        &self,
        limit: usize,
    ) -> Result<Vec<PlannerCodexSession>, String> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT session_id, session_name, cwd, sandbox, updated_at_ms, status,
                        task_summary, last_result_summary
                 FROM codex_sessions
                 ORDER BY updated_at_ms DESC
                 LIMIT ?1",
            )
            .map_err(|err| format!("prepare codex planner sessions failed: {err}"))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let updated_at_ms = row.get::<_, i64>(4)? as u64;
                let updated_at_utc = Utc
                    .timestamp_millis_opt(updated_at_ms as i64)
                    .single()
                    .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
                    .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
                Ok(PlannerCodexSession {
                    session_id: row.get(0)?,
                    session_name: row.get(1)?,
                    cwd: row.get(2)?,
                    sandbox: parse_sandbox(&row.get::<_, String>(3)?)?,
                    updated_at_utc,
                    status: row.get(5)?,
                    task_summary: row.get(6)?,
                    last_result_summary: row.get(7)?,
                })
            })
            .map_err(|err| format!("query codex planner sessions failed: {err}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("read codex planner session failed: {err}"))?);
        }
        Ok(out)
    }

    fn init(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("create codex store dir failed: {err}"))?;
        }
        let conn = self.open()?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS codex_sessions (
                session_id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                session_name TEXT NOT NULL,
                cwd TEXT NOT NULL,
                sandbox TEXT NOT NULL,
                task_summary TEXT NOT NULL,
                last_result_summary TEXT,
                status TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS codex_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                turn_id TEXT,
                instruction TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL,
                user_visible TEXT,
                created_at_ms INTEGER NOT NULL,
                completed_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS codex_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                turn_id TEXT,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
             );",
        )
        .map_err(|err| format!("initialize codex store failed: {err}"))?;
        Ok(())
    }

    fn open(&self) -> Result<Connection, String> {
        Connection::open(&self.path).map_err(|err| {
            format!(
                "open codex sqlite store {} failed: {err}",
                self.path.display()
            )
        })
    }
}

fn parse_sandbox(raw: &str) -> rusqlite::Result<CodexSandboxMode> {
    match raw {
        "read_only" => Ok(CodexSandboxMode::ReadOnly),
        "workspace_write" => Ok(CodexSandboxMode::WorkspaceWrite),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("invalid sandbox `{other}`").into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("duration")
            .as_nanos();
        std::env::temp_dir().join(format!("sieve-codex-store-{unique}.db"))
    }

    #[test]
    fn session_round_trip_and_planner_projection() {
        let path = temp_store_path();
        let store = CodexSessionStore::new(&path).expect("create store");
        store
            .upsert_session(&StoredCodexSession {
                session_id: "fix-auth-flow".to_string(),
                thread_id: "thr_123".to_string(),
                session_name: "fix-auth-flow".to_string(),
                cwd: "/tmp/repo".to_string(),
                sandbox: CodexSandboxMode::WorkspaceWrite,
                task_summary: "fix auth flow".to_string(),
                last_result_summary: Some("updated parser".to_string()),
                status: "completed".to_string(),
                created_at_ms: 1,
                updated_at_ms: 2,
            })
            .expect("upsert session");
        let loaded = store
            .session("fix-auth-flow")
            .expect("load session")
            .expect("stored session");
        assert_eq!(loaded.thread_id, "thr_123");
        let planner = store.planner_sessions(5).expect("planner sessions");
        assert_eq!(planner.len(), 1);
        assert_eq!(planner[0].session_id, "fix-auth-flow");
        assert_eq!(planner[0].sandbox, CodexSandboxMode::WorkspaceWrite);
        let _ = std::fs::remove_file(path);
    }
}
