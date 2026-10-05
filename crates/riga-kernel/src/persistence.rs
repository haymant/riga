use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    events::RigaEventEnvelope,
    state::{RigaError, RigaErrorCode, Session},
};

fn persistence_error(message: impl Into<String>) -> RigaError {
    RigaError {
        code: RigaErrorCode::PersistenceError,
        message: message.into(),
        retryable: true,
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), RigaError> {
    let content =
        serde_json::to_vec_pretty(value).map_err(|error| persistence_error(error.to_string()))?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, content).map_err(|error| persistence_error(error.to_string()))?;
    fs::rename(&temporary, path).map_err(|error| persistence_error(error.to_string()))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, RigaError> {
    let content = fs::read(path).map_err(|error| persistence_error(error.to_string()))?;
    serde_json::from_slice(&content).map_err(|error| persistence_error(error.to_string()))
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, RigaError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|error| persistence_error(error.to_string()))?;
        Ok(Self { root })
    }

    fn path_for(&self, id: &str) -> Result<PathBuf, RigaError> {
        if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
            return Err(RigaError {
                code: RigaErrorCode::InvalidRequest,
                message: "invalid generated session identifier".into(),
                retryable: false,
            });
        }
        Ok(self.root.join(format!("session-{id}.json")))
    }

    pub fn save(&self, session: &Session) -> Result<(), RigaError> {
        write_json(&self.path_for(&session.id)?, session)
    }

    pub fn get(&self, id: &str) -> Result<Option<Session>, RigaError> {
        let path = self.path_for(id)?;
        if !path.exists() {
            return Ok(None);
        }
        read_json(&path).map(Some)
    }

    pub fn list(&self) -> Result<Vec<Session>, RigaError> {
        let mut sessions: Vec<Session> = Vec::new();
        for entry in
            fs::read_dir(&self.root).map_err(|error| persistence_error(error.to_string()))?
        {
            let path = entry
                .map_err(|error| persistence_error(error.to_string()))?
                .path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("session-") && name.ends_with(".json"))
            {
                sessions.push(read_json(&path)?);
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }
}

#[derive(Debug, Clone)]
pub struct EventJournal {
    path: PathBuf,
    events: Vec<RigaEventEnvelope>,
}

impl EventJournal {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, RigaError> {
        let path = path.into();
        let events = if path.exists() {
            read_json(&path)?
        } else {
            Vec::new()
        };
        Ok(Self { path, events })
    }

    pub fn append(&mut self, event: RigaEventEnvelope) -> Result<(), RigaError> {
        if self
            .events
            .iter()
            .any(|existing| existing.event_id == event.event_id)
        {
            return Ok(());
        }
        if self
            .events
            .last()
            .is_some_and(|existing| event.sequence <= existing.sequence)
        {
            return Err(persistence_error(
                "event sequence must increase monotonically",
            ));
        }
        self.events.push(event);
        write_json(&self.path, &self.events)
    }

    pub fn all(&self) -> &[RigaEventEnvelope] {
        &self.events
    }

    pub fn after_sequence(&self, sequence: u64) -> Vec<RigaEventEnvelope> {
        self.events
            .iter()
            .filter(|event| event.sequence > sequence)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{EventJournal, SessionStore};
    use crate::{
        events::{RigaEvent, RigaEventEnvelope},
        state::Session,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("riga-{label}-{stamp}"))
    }

    fn session(id: &str) -> Session {
        Session {
            id: id.into(),
            title: "Test".into(),
            workspace: "/workspace".into(),
            created_at: "now".into(),
            updated_at: "now".into(),
        }
    }

    fn event(id: &str, sequence: u64) -> RigaEventEnvelope {
        RigaEventEnvelope {
            protocol_version: 1,
            event_id: id.into(),
            session_id: "session-1".into(),
            run_id: "run-1".into(),
            sequence,
            timestamp: "now".into(),
            event: RigaEvent::RunStarted,
        }
    }

    #[test]
    fn session_store_round_trips_and_lists() {
        let root = temp_dir("sessions");
        let store = SessionStore::open(&root).unwrap();
        store.save(&session("one")).unwrap();
        assert_eq!(store.get("one").unwrap().unwrap().title, "Test");
        assert_eq!(store.list().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn journal_replays_after_cursor_and_deduplicates() {
        let root = temp_dir("journal");
        let mut journal = EventJournal::open(&root).unwrap();
        journal.append(event("one", 1)).unwrap();
        journal.append(event("one", 1)).unwrap();
        journal.append(event("two", 2)).unwrap();
        assert_eq!(journal.all().len(), 2);
        assert_eq!(journal.after_sequence(1).len(), 1);
        let _ = std::fs::remove_file(root);
    }

    #[test]
    fn corrupted_journal_fails_closed_and_out_of_order_events_are_rejected() {
        let root = temp_dir("corrupt-journal");
        std::fs::write(&root, b"{truncated").unwrap();
        let error = EventJournal::open(&root).expect_err("corrupt journal must not be accepted");
        assert_eq!(error.code, crate::state::RigaErrorCode::PersistenceError);

        let clean = temp_dir("ordering-journal");
        let mut journal = EventJournal::open(&clean).unwrap();
        journal.append(event("one", 2)).unwrap();
        let error = journal
            .append(event("two", 1))
            .expect_err("sequence must increase");
        assert_eq!(error.code, crate::state::RigaErrorCode::PersistenceError);
        let _ = std::fs::remove_file(root);
        let _ = std::fs::remove_file(clean);
    }
}
