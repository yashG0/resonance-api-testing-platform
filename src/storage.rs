//! Local SQLite persistence for collections, variables, and saved requests.
//!
//! SQLite work is synchronous and should run on a background worker, not on the
//! Slint event loop.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionVariable {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRequest {
    pub id: i64,
    pub collection_id: i64,
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers: String,
    pub query_params: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRequest {
    pub collection_id: i64,
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
    #[error("invalid data: {0}")]
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
            PRAGMA journal_mode = WAL;

            CREATE TABLE IF NOT EXISTS collections (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                name       TEXT NOT NULL COLLATE NOCASE UNIQUE,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS saved_requests (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT NOT NULL,
                method       TEXT NOT NULL,
                url          TEXT NOT NULL,
                headers      TEXT NOT NULL DEFAULT '',
                query_params TEXT NOT NULL DEFAULT '',
                body         TEXT NOT NULL DEFAULT '',
                collection_id INTEGER REFERENCES collections(id) ON DELETE CASCADE,
                created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS collection_variables (
                collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
                key           TEXT NOT NULL,
                value         TEXT NOT NULL,
                PRIMARY KEY (collection_id, key)
            );

            CREATE INDEX IF NOT EXISTS idx_saved_requests_name
                ON saved_requests(name COLLATE NOCASE);
            "#,
        )?;

        // Keep existing databases usable: earlier versions stored requests
        // without a collection. Add the column and place those requests in the
        // first default collection while retaining their IDs and contents.
        let has_collection_id = {
            let mut statement = connection.prepare("PRAGMA table_info(saved_requests)")?;
            let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
            columns
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|column| column == "collection_id")
        };
        if !has_collection_id {
            connection.execute(
                "ALTER TABLE saved_requests ADD COLUMN collection_id INTEGER REFERENCES collections(id) ON DELETE CASCADE",
                [],
            )?;
        }

        connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_saved_requests_collection ON saved_requests(collection_id);",
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO collections (name) VALUES (?1), (?2)",
            params!["Public API", "My Backend"],
        )?;
        let default_id = connection.query_row(
            "SELECT id FROM collections ORDER BY id ASC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        connection.execute(
            "UPDATE saved_requests SET collection_id = ?1 WHERE collection_id IS NULL",
            [default_id],
        )?;

        Ok(store)
    }

    fn connect(&self) -> Result<Connection, StoreError> {
        let connection = Connection::open(&self.database_path)?;
        connection.busy_timeout(std::time::Duration::from_secs(3))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(connection)
    }

    pub fn list_collections(&self) -> Result<Vec<Collection>, StoreError> {
        let connection = self.connect()?;
        let mut statement =
            connection.prepare("SELECT id, name FROM collections ORDER BY id ASC")?;
        let rows = statement.query_map([], |row| {
            Ok(Collection {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn create_collection(&self, name: &str) -> Result<Collection, StoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Validation(
                "collection name cannot be empty".into(),
            ));
        }

        let connection = self.connect()?;
        let exists = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE name = ?1)",
            [name],
            |row| row.get::<_, bool>(0),
        )?;
        if exists {
            return Err(StoreError::Validation(format!(
                "collection '{name}' already exists"
            )));
        }

        connection.execute("INSERT INTO collections (name) VALUES (?1)", [name])?;
        let id = connection.last_insert_rowid();
        Ok(Collection {
            id,
            name: name.to_owned(),
        })
    }

    pub fn rename_collection(
        &self,
        collection_id: i64,
        name: &str,
    ) -> Result<Collection, StoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::Validation(
                "collection name cannot be empty".into(),
            ));
        }

        let connection = self.connect()?;
        let exists = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)",
            [collection_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(StoreError::Validation("collection no longer exists".into()));
        }

        let duplicate = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE name = ?1 AND id != ?2)",
            params![name, collection_id],
            |row| row.get::<_, bool>(0),
        )?;
        if duplicate {
            return Err(StoreError::Validation(format!(
                "collection '{name}' already exists"
            )));
        }

        connection.execute(
            "UPDATE collections SET name = ?1 WHERE id = ?2",
            params![name, collection_id],
        )?;
        Ok(Collection {
            id: collection_id,
            name: name.to_owned(),
        })
    }

    /// Deletes a collection and its requests and variables. At least one
    /// collection is always kept so the app has a valid destination for new requests.
    pub fn delete_collection(&self, collection_id: i64) -> Result<bool, StoreError> {
        let connection = self.connect()?;
        let count = connection.query_row("SELECT COUNT(*) FROM collections", [], |row| {
            row.get::<_, i64>(0)
        })?;
        if count <= 1 {
            return Err(StoreError::Validation(
                "the last collection cannot be deleted".into(),
            ));
        }

        let deleted =
            connection.execute("DELETE FROM collections WHERE id = ?1", [collection_id])?;
        if deleted == 0 {
            return Err(StoreError::Validation("collection no longer exists".into()));
        }
        Ok(true)
    }

    pub fn list_variables(
        &self,
        collection_id: i64,
    ) -> Result<Vec<CollectionVariable>, StoreError> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT key, value FROM collection_variables WHERE collection_id = ?1 ORDER BY key COLLATE NOCASE ASC",
        )?;
        let rows = statement.query_map([collection_id], |row| {
            Ok(CollectionVariable {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn variables_map(&self, collection_id: i64) -> Result<HashMap<String, String>, StoreError> {
        Ok(self
            .list_variables(collection_id)?
            .into_iter()
            .map(|v| (v.key, v.value))
            .collect())
    }

    pub fn upsert_variable(
        &self,
        collection_id: i64,
        key: &str,
        value: &str,
    ) -> Result<(), StoreError> {
        let key = key.trim();
        if !valid_variable_key(key) {
            return Err(StoreError::Validation(
                "variable names must start with a letter or underscore and contain only letters, numbers, or underscores".into(),
            ));
        }

        let connection = self.connect()?;
        connection.execute(
            r#"
            INSERT INTO collection_variables (collection_id, key, value)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(collection_id, key) DO UPDATE SET value = excluded.value
            "#,
            params![collection_id, key, value],
        )?;
        Ok(())
    }

    pub fn delete_variable(&self, collection_id: i64, key: &str) -> Result<bool, StoreError> {
        let connection = self.connect()?;
        let changed = connection.execute(
            "DELETE FROM collection_variables WHERE collection_id = ?1 AND key = ?2",
            params![collection_id, key],
        )?;
        Ok(changed == 1)
    }

    pub fn create(&self, request: &NewRequest) -> Result<SavedRequest, StoreError> {
        validate_request(request)?;
        let connection = self.connect()?;
        connection.execute(
            r#"
            INSERT INTO saved_requests (name, method, url, headers, query_params, body, collection_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                request.name.trim(),
                request.method.trim().to_ascii_uppercase(),
                request.url.trim(),
                request.headers,
                request.query_params,
                request.body,
                request.collection_id
            ],
        )?;
        let id = connection.last_insert_rowid();
        self.get(id)?.ok_or_else(|| {
            StoreError::Validation("request was inserted but could not be read back".into())
        })
    }

    pub fn list_for_collection(&self, collection_id: i64) -> Result<Vec<SavedRequest>, StoreError> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT id, collection_id, name, method, url, headers, query_params, body
            FROM saved_requests WHERE collection_id = ?1
            ORDER BY name COLLATE NOCASE ASC, id ASC
            "#,
        )?;
        let rows = statement.query_map([collection_id], row_to_request)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn get(&self, id: i64) -> Result<Option<SavedRequest>, StoreError> {
        let connection = self.connect()?;
        connection
            .query_row(
                r#"
                SELECT id, collection_id, name, method, url, headers, query_params, body
                FROM saved_requests WHERE id = ?1
                "#,
                [id],
                row_to_request,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn update(&self, id: i64, request: &NewRequest) -> Result<bool, StoreError> {
        validate_request(request)?;
        let connection = self.connect()?;
        let changed = connection.execute(
            r#"
            UPDATE saved_requests
            SET name = ?1, method = ?2, url = ?3, headers = ?4,
                query_params = ?5, body = ?6, collection_id = ?7,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ?8
            "#,
            params![
                request.name.trim(),
                request.method.trim().to_ascii_uppercase(),
                request.url.trim(),
                request.headers,
                request.query_params,
                request.body,
                request.collection_id,
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

fn valid_variable_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn validate_request(request: &NewRequest) -> Result<(), StoreError> {
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
        collection_id: row.get(1)?,
        name: row.get(2)?,
        method: row.get(3)?,
        url: row.get(4)?,
        headers: row.get(5)?,
        query_params: row.get(6)?,
        body: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(collection_id: i64) -> NewRequest {
        NewRequest {
            collection_id,
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
        let collection_id = store.list_collections().expect("collections")[0].id;

        let created = store
            .create(&sample(collection_id))
            .expect("create request");
        assert!(created.id > 0);
        assert_eq!(created.name, "Get Users");
        assert_eq!(created.method, "GET");
        assert_eq!(created.collection_id, collection_id);
        assert_eq!(
            store.get(created.id).expect("get request"),
            Some(created.clone())
        );

        let updated = NewRequest {
            collection_id,
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
        assert_eq!(
            store
                .list_for_collection(collection_id)
                .expect("list collection")
                .len(),
            1
        );

        assert!(store.delete(created.id).expect("delete"));
        assert!(store.get(created.id).expect("read deleted").is_none());
    }

    #[test]
    fn collection_variables_are_isolated_and_upserted() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = RequestStore::open(temp.path().join("test.sqlite3")).expect("open store");
        let first = store.list_collections().expect("collections")[0].id;
        let second = store
            .create_collection("Staging")
            .expect("create collection");

        store
            .upsert_variable(first, "base_url", "https://api.example.com")
            .expect("add variable");
        store
            .upsert_variable(first, "base_url", "https://staging.example.com")
            .expect("update variable");
        store
            .upsert_variable(second.id, "base_url", "https://stage.example.com")
            .expect("add scoped variable");

        assert_eq!(store.list_variables(first).expect("list vars").len(), 1);
        assert_eq!(
            store.variables_map(first).expect("map vars")["base_url"],
            "https://staging.example.com"
        );
        assert_eq!(
            store
                .variables_map(second.id)
                .expect("other collection vars")["base_url"],
            "https://stage.example.com"
        );
        assert!(
            store
                .delete_variable(first, "base_url")
                .expect("delete var")
        );
        assert!(
            store
                .list_variables(first)
                .expect("list after delete")
                .is_empty()
        );
    }

    #[test]
    fn collection_rename_and_delete_preserve_collection_invariants() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = RequestStore::open(temp.path().join("test.sqlite3")).expect("open store");
        let initial_collections = store.list_collections().expect("collections");
        let default = initial_collections[0].clone();
        let last_seeded = initial_collections[1].clone();
        let second = store
            .create_collection("Staging")
            .expect("create collection");

        let renamed = store
            .rename_collection(second.id, "  Preview  ")
            .expect("rename collection");
        assert_eq!(renamed.name, "Preview");
        assert!(matches!(
            store.rename_collection(second.id, &default.name),
            Err(StoreError::Validation(_))
        ));
        assert!(matches!(
            store.rename_collection(second.id, "   "),
            Err(StoreError::Validation(_))
        ));

        store
            .upsert_variable(second.id, "token", "secret")
            .expect("add collection variable");
        store.create(&sample(second.id)).expect("create request");
        assert!(
            store
                .delete_collection(second.id)
                .expect("delete collection")
        );
        assert!(
            store
                .list_variables(second.id)
                .expect("list variables")
                .is_empty()
        );
        assert!(
            store
                .list_for_collection(second.id)
                .expect("list requests")
                .is_empty()
        );
        assert_eq!(
            store.list_collections().expect("seeded collections").len(),
            initial_collections.len()
        );
        assert!(store.delete_collection(default.id).expect("delete default"));
        assert!(matches!(
            store.delete_collection(last_seeded.id),
            Err(StoreError::Validation(_))
        ));
    }

    #[test]
    fn migrates_existing_requests_into_default_collection() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("legacy.sqlite3");
        let connection = Connection::open(&path).expect("open legacy database");
        connection.execute_batch(
            r#"
            CREATE TABLE saved_requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                method TEXT NOT NULL,
                url TEXT NOT NULL,
                headers TEXT NOT NULL DEFAULT '',
                query_params TEXT NOT NULL DEFAULT '',
                body TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            INSERT INTO saved_requests (name, method, url) VALUES ('Legacy', 'GET', 'https://example.com');
            "#,
        ).expect("create old schema");
        drop(connection);

        let store = RequestStore::open(path).expect("migrate old database");
        let collection_id = store.list_collections().expect("collections")[0].id;
        let migrated = store
            .list_for_collection(collection_id)
            .expect("list migrated");
        assert_eq!(migrated.len(), 1);
        assert_eq!(migrated[0].name, "Legacy");
        assert_eq!(migrated[0].collection_id, collection_id);
    }

    #[test]
    fn rejects_invalid_variable_keys() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = RequestStore::open(temp.path().join("test.sqlite3")).expect("open store");
        let collection_id = store.list_collections().expect("collections")[0].id;
        assert!(matches!(
            store.upsert_variable(collection_id, "api-key", "secret"),
            Err(StoreError::Validation(_))
        ));
    }

    #[test]
    fn rejects_empty_request_name() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = RequestStore::open(temp.path().join("test.sqlite3")).expect("open store");
        let mut request = sample(store.list_collections().expect("collections")[0].id);
        request.name = "  ".into();
        assert!(matches!(
            store.create(&request),
            Err(StoreError::Validation(_))
        ));
    }
}
