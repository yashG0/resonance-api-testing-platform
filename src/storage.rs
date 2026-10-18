//! Local SQLite persistence for saved Resonance requests.
//!
//! This module is intentionally synchronous: SQLite operations are short and are
//! expected to be called from the UI's background worker, not from the Slint event loop.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRequest {
    pub id: i64,
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers: String,
    pub query_params: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRequest {
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers: String,
    pub query_params: String,
    pub body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not resolve the application data directory")]
    NoDataDirectory,
    #[error("invalid request: {0}")]
    Validation(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct RequestStore {
    database_path: PathBuf,
}

impl RequestStore {
    /// Opens the application's normal per-user database, creating it if needed.
    pub fn open_default() -> Result<Self, StoreError> {
        let project_dirs = ProjectDirs::from("dev", "Resonance", "Resonance")
            .ok_or(StoreError::NoDataDirectory)?;
        let data_dir = project_dirs.data_local_dir();
        std::fs::create_dir_all(data_dir)?;
        Self::open(data_dir.join("requests.sqlite3"))
    }

    /// Opens a database at an explicit path. Useful for tests and portable setups.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let database_path = path.as_ref().to_path_buf();
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let store = Self { database_path };
        let connection = store.connect()?;
        connection.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;

            CREATE TABLE IF NOT EXISTS saved_requests (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT NOT NULL,
                method       TEXT NOT NULL,
                url          TEXT NOT NULL,
                headers      TEXT NOT NULL DEFAULT '',
                query_params TEXT NOT NULL DEFAULT '',
                body         TEXT NOT NULL DEFAULT '',
                created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_saved_requests_name
                ON saved_requests(name COLLATE NOCASE);
            "#,
        )?;
        Ok(store)
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    fn connect(&self) -> Result<Connection, StoreError> {
        let connection = Connection::open(&self.database_path)?;
        connection.busy_timeout(std::time::Duration::from_secs(3))?;
        Ok(connection)
    }

    pub fn create(&self, request: &NewRequest) -> Result<SavedRequest, StoreError> {
        validate(request)?;
        let connection = self.connect()?;
        connection.execute(
            r#"
            INSERT INTO saved_requests (name, method, url, headers, query_params, body)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                request.name.trim(),
                request.method.trim().to_ascii_uppercase(),
                request.url.trim(),
                request.headers,
                request.query_params,
                request.body
            ],
        )?;
        let id = connection.last_insert_rowid();
        self.get(id)?.ok_or_else(|| {
            StoreError::Validation("request was inserted but could not be read back".into())
        })
    }

    pub fn list(&self) -> Result<Vec<SavedRequest>, StoreError> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT id, name, method, url, headers, query_params, body
            FROM saved_requests
            ORDER BY name COLLATE NOCASE ASC, id ASC
            "#,
        )?;
        let rows = statement.query_map([], row_to_request)?;
        let mut requests = Vec::new();
        for row in rows {
            requests.push(row?);
        }
        Ok(requests)
    }

    pub fn get(&self, id: i64) -> Result<Option<SavedRequest>, StoreError> {
        let connection = self.connect()?;
        connection
            .query_row(
                r#"
                SELECT id, name, method, url, headers, query_params, body
                FROM saved_requests WHERE id = ?1
                "#,
                [id],
                row_to_request,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn update(&self, id: i64, request: &NewRequest) -> Result<bool, StoreError> {
        validate(request)?;
        let connection = self.connect()?;
        let changed = connection.execute(
            r#"
            UPDATE saved_requests
            SET name = ?1, method = ?2, url = ?3, headers = ?4,
                query_params = ?5, body = ?6, updated_at = CURRENT_TIMESTAMP
            WHERE id = ?7
            "#,
            params![
                request.name.trim(),
                request.method.trim().to_ascii_uppercase(),
                request.url.trim(),
                request.headers,
                request.query_params,
                request.body,
                id
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn delete(&self, id: i64) -> Result<bool, StoreError> {
        let connection = self.connect()?;
        let changed = connection.execute("DELETE FROM saved_requests WHERE id = ?1", [id])?;
        Ok(changed == 1)
    }
}

fn validate(request: &NewRequest) -> Result<(), StoreError> {
    if request.name.trim().is_empty() {
        return Err(StoreError::Validation(
            "request name cannot be empty".into(),
        ));
    }
    if request.url.trim().is_empty() {
        return Err(StoreError::Validation("request URL cannot be empty".into()));
    }

    let method = request.method.trim().to_ascii_uppercase();
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return Err(StoreError::Validation(format!(
            "unsupported HTTP method: {method}"
        )));
    }

    Ok(())
}

fn row_to_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedRequest> {
    Ok(SavedRequest {
        id: row.get(0)?,
        name: row.get(1)?,
        method: row.get(2)?,
        url: row.get(3)?,
        headers: row.get(4)?,
        query_params: row.get(5)?,
        body: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NewRequest {
        NewRequest {
            name: "Get Users".into(),
            method: "GET".into(),
            url: "https://httpbin.org/get".into(),
            headers: "Accept: application/json".into(),
            query_params: "page=1".into(),
            body: String::new(),
        }
    }

    #[test]
    fn create_read_update_delete_round_trip() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = RequestStore::open(temp.path().join("test.sqlite3")).expect("open store");

        let created = store.create(&sample()).expect("create request");
        assert!(created.id > 0);
        assert_eq!(created.name, "Get Users");
        assert_eq!(created.method, "GET");
        assert_eq!(
            store.get(created.id).expect("get request"),
            Some(created.clone())
        );

        let updated = NewRequest {
            name: "Updated Users".into(),
            method: "POST".into(),
            url: "https://httpbin.org/post".into(),
            headers: "Content-Type: application/json".into(),
            query_params: String::new(),
            body: r#"{"ok":true}"#.into(),
        };
        assert!(store.update(created.id, &updated).expect("update"));
        let loaded = store
            .get(created.id)
            .expect("read updated")
            .expect("exists");
        assert_eq!(loaded.name, "Updated Users");
        assert_eq!(loaded.method, "POST");
        assert_eq!(loaded.body, r#"{"ok":true}"#);

        assert_eq!(store.list().expect("list").len(), 1);
        assert!(store.delete(created.id).expect("delete"));
        assert!(store.get(created.id).expect("read deleted").is_none());
    }

    #[test]
    fn rejects_empty_name() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = RequestStore::open(temp.path().join("test.sqlite3")).expect("open store");
        let mut request = sample();
        request.name = "  ".into();
        assert!(matches!(
            store.create(&request),
            Err(StoreError::Validation(_))
        ));
    }
}
