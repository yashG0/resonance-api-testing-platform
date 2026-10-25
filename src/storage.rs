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
use uuid::Uuid;

use crate::credentials::{
    CredentialVault, CredentialVaultError, RequestCredentials, SystemCredentialVault,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHistory {
    pub request_id: i64,
    pub collection_name: String,
    pub request_name: String,
    pub method: String,
    pub url: String,
    pub status: String,
    pub elapsed_ms: i64,
    pub created_at: String,
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
    pub body_format: String,
    pub auth_type: String,
    pub auth_value: String,
    pub auth_secret: String,
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
    pub body_format: String,
    pub auth_type: String,
    pub auth_value: String,
    pub auth_secret: String,
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
    #[error(transparent)]
    CredentialVault(#[from] CredentialVaultError),
}

#[derive(Clone)]
pub struct RequestStore {
    database_path: PathBuf,
    credential_vault: std::sync::Arc<dyn CredentialVault>,
}

struct StoredSavedRequest {
    request: SavedRequest,
    credential_ref: String,
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
        Self::open_with_vault(path, std::sync::Arc::new(SystemCredentialVault))
    }

    fn open_with_vault(
        path: impl AsRef<Path>,
        credential_vault: std::sync::Arc<dyn CredentialVault>,
    ) -> Result<Self, StoreError> {
        let database_path = path.as_ref().to_path_buf();
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let store = Self {
            database_path,
            credential_vault,
        };
        let mut connection = store.connect()?;
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
                body_format  TEXT NOT NULL DEFAULT 'JSON',
                auth_type    TEXT NOT NULL DEFAULT 'None',
                auth_value   TEXT NOT NULL DEFAULT '',
                auth_secret  TEXT NOT NULL DEFAULT '',
                credential_ref TEXT NOT NULL DEFAULT '',
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

            CREATE TABLE IF NOT EXISTS request_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id INTEGER NOT NULL,
                collection_name TEXT NOT NULL,
                request_name TEXT NOT NULL,
                method TEXT NOT NULL,
                url TEXT NOT NULL,
                status TEXT NOT NULL,
                elapsed_ms INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
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

        // Add request authentication fields to databases created by earlier versions.
        for (column, declaration) in [
            ("auth_type", "TEXT NOT NULL DEFAULT 'None'"),
            ("body_format", "TEXT NOT NULL DEFAULT 'JSON'"),
            ("auth_value", "TEXT NOT NULL DEFAULT ''"),
            ("auth_secret", "TEXT NOT NULL DEFAULT ''"),
            ("credential_ref", "TEXT NOT NULL DEFAULT ''"),
        ] {
            let has_column = {
                let mut statement = connection.prepare("PRAGMA table_info(saved_requests)")?;
                let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
                columns
                    .collect::<Result<Vec<_>, _>>()?
                    .iter()
                    .any(|item| item == column)
            };
            if !has_column {
                connection.execute_batch(&format!(
                    "ALTER TABLE saved_requests ADD COLUMN {column} {declaration}"
                ))?;
            }
        }

        connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_saved_requests_collection ON saved_requests(collection_id);",
        )?;
        // Public API is the only built-in collection. Older versions also
        // created a default "My Backend" collection; merge its contents into
        // Public API so upgrading does not discard saved requests or variables.
        connection.execute(
            "INSERT OR IGNORE INTO collections (name) VALUES (?1)",
            ["Public API"],
        )?;
        let default_id = connection.query_row(
            "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE",
            ["Public API"],
            |row| row.get::<_, i64>(0),
        )?;

        let legacy_default_id = connection
            .query_row(
                "SELECT id FROM collections WHERE name = ?1 COLLATE NOCASE",
                ["My Backend"],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(legacy_id) = legacy_default_id {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE saved_requests SET collection_id = ?1 WHERE collection_id = ?2",
                params![default_id, legacy_id],
            )?;
            // Keep existing Public API values when both collections define the
            // same variable; otherwise carry the legacy value forward.
            transaction.execute(
                "INSERT OR IGNORE INTO collection_variables (collection_id, key, value)
                 SELECT ?1, key, value FROM collection_variables WHERE collection_id = ?2",
                params![default_id, legacy_id],
            )?;
            transaction.execute("DELETE FROM collections WHERE id = ?1", [legacy_id])?;
            transaction.commit()?;
        }
        connection.execute(
            "UPDATE saved_requests SET collection_id = ?1 WHERE collection_id IS NULL",
            [default_id],
        )?;

        store.migrate_plaintext_credentials(&mut connection)?;
        Ok(store)
    }

    fn connect(&self) -> Result<Connection, StoreError> {
        let connection = Connection::open(&self.database_path)?;
        connection.busy_timeout(std::time::Duration::from_secs(3))?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
        Ok(connection)
    }

    fn store_request_credentials(&self, request: &NewRequest) -> Result<String, StoreError> {
        if request.auth_type == "None"
            || (request.auth_value.is_empty() && request.auth_secret.is_empty())
        {
            return Ok(String::new());
        }

        let reference = Uuid::new_v4().to_string();
        self.credential_vault.store(
            &reference,
            &RequestCredentials {
                auth_value: request.auth_value.clone(),
                auth_secret: request.auth_secret.clone(),
            },
        )?;
        Ok(reference)
    }

    fn remove_request_credentials(&self, reference: &str) {
        if !reference.is_empty()
            && let Err(error) = self.credential_vault.delete(reference)
        {
            eprintln!("Resonance: could not remove an unused credential entry: {error}");
        }
    }

    fn hydrate_request(&self, mut stored: StoredSavedRequest) -> Result<SavedRequest, StoreError> {
        if !stored.credential_ref.is_empty() {
            let credentials = self
                .credential_vault
                .load(&stored.credential_ref)?
                .ok_or(CredentialVaultError::Missing)?;
            stored.request.auth_value = credentials.auth_value;
            stored.request.auth_secret = credentials.auth_secret;
        }
        Ok(stored.request)
    }

    /// Move credentials from legacy SQLite columns into the system credential
    /// store, then scrub the old values from the database and WAL file.
    fn migrate_plaintext_credentials(&self, connection: &mut Connection) -> Result<(), StoreError> {
        let legacy_rows = {
            let mut statement = connection.prepare(
                "SELECT id, auth_value, auth_secret, credential_ref FROM saved_requests WHERE auth_value != '' OR auth_secret != ''",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        if legacy_rows.is_empty() {
            return Ok(());
        }

        let mut migrated = Vec::with_capacity(legacy_rows.len());
        for (id, auth_value, auth_secret, existing_ref) in legacy_rows {
            let credential_ref = if existing_ref.is_empty() {
                format!("legacy-request-{id}")
            } else {
                existing_ref
            };
            self.credential_vault.store(
                &credential_ref,
                &RequestCredentials {
                    auth_value,
                    auth_secret,
                },
            )?;
            migrated.push((id, credential_ref));
        }

        let transaction = connection.transaction()?;
        for (id, credential_ref) in migrated {
            transaction.execute(
                "UPDATE saved_requests SET auth_value = '', auth_secret = '', credential_ref = ?1 WHERE id = ?2",
                params![credential_ref, id],
            )?;
        }
        transaction.commit()?;
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        Ok(())
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

        let credential_refs = {
            let mut statement = connection
                .prepare("SELECT credential_ref FROM saved_requests WHERE collection_id = ?1")?;
            let rows = statement.query_map([collection_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let deleted =
            connection.execute("DELETE FROM collections WHERE id = ?1", [collection_id])?;
        if deleted == 0 {
            return Err(StoreError::Validation("collection no longer exists".into()));
        }
        drop(connection);
        for reference in credential_refs {
            self.remove_request_credentials(&reference);
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
        let credential_ref = self.store_request_credentials(request)?;
        let connection = self.connect()?;
        let insert_result = connection.execute(
            r#"
            INSERT INTO saved_requests (name, method, url, headers, query_params, body, collection_id, body_format, auth_type, auth_value, auth_secret, credential_ref)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '', '', ?10)
            "#,
            params![
                request.name.trim(),
                request.method.trim().to_ascii_uppercase(),
                request.url.trim(),
                request.headers,
                request.query_params,
                request.body,
                request.collection_id,
                request.body_format,
                request.auth_type,
                credential_ref
            ],
        );
        if let Err(error) = insert_result {
            drop(connection);
            self.remove_request_credentials(&credential_ref);
            return Err(error.into());
        }
        let id = connection.last_insert_rowid();
        drop(connection);
        self.get(id)?.ok_or_else(|| {
            StoreError::Validation("request was inserted but could not be read back".into())
        })
    }

    pub fn list_for_collection(&self, collection_id: i64) -> Result<Vec<SavedRequest>, StoreError> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            r#"
            SELECT id, collection_id, name, method, url, headers, query_params, body, body_format, auth_type, auth_value, auth_secret, credential_ref
            FROM saved_requests WHERE collection_id = ?1
            ORDER BY name COLLATE NOCASE ASC, id ASC
            "#,
        )?;
        let rows = statement.query_map([collection_id], row_to_stored_request)?;
        let stored = rows.collect::<Result<Vec<_>, _>>()?;
        stored
            .into_iter()
            .map(|request| self.hydrate_request(request))
            .collect()
    }

    pub fn get(&self, id: i64) -> Result<Option<SavedRequest>, StoreError> {
        let connection = self.connect()?;
        let stored = connection
            .query_row(
                r#"
                SELECT id, collection_id, name, method, url, headers, query_params, body, body_format, auth_type, auth_value, auth_secret, credential_ref
                FROM saved_requests WHERE id = ?1
                "#,
                [id],
                row_to_stored_request,
            )
            .optional()
            ?;
        stored
            .map(|request| self.hydrate_request(request))
            .transpose()
    }

    pub fn update(&self, id: i64, request: &NewRequest) -> Result<bool, StoreError> {
        validate_request(request)?;
        let credential_ref = self.store_request_credentials(request)?;
        let connection = self.connect()?;
        let old_ref = connection
            .query_row(
                "SELECT credential_ref FROM saved_requests WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(old_ref) = old_ref else {
            drop(connection);
            self.remove_request_credentials(&credential_ref);
            return Ok(false);
        };
        let update_result = connection.execute(
            r#"
            UPDATE saved_requests
            SET name = ?1, method = ?2, url = ?3, headers = ?4,
                query_params = ?5, body = ?6, collection_id = ?7,
                body_format = ?8, auth_type = ?9, auth_value = '', auth_secret = '',
                credential_ref = ?10, updated_at = CURRENT_TIMESTAMP
            WHERE id = ?11
            "#,
            params![
                request.name.trim(),
                request.method.trim().to_ascii_uppercase(),
                request.url.trim(),
                request.headers,
                request.query_params,
                request.body,
                request.collection_id,
                request.body_format,
                request.auth_type,
                credential_ref,
                id
            ],
        );
        let changed = match update_result {
            Ok(changed) => changed,
            Err(error) => {
                drop(connection);
                self.remove_request_credentials(&credential_ref);
                return Err(error.into());
            }
        };
        drop(connection);
        if changed == 1 {
            self.remove_request_credentials(&old_ref);
            Ok(true)
        } else {
            self.remove_request_credentials(&credential_ref);
            Ok(false)
        }
    }

    pub fn delete(&self, id: i64) -> Result<bool, StoreError> {
        let connection = self.connect()?;
        let credential_ref = connection
            .query_row(
                "SELECT credential_ref FROM saved_requests WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let changed = connection.execute("DELETE FROM saved_requests WHERE id = ?1", [id])?;
        drop(connection);
        if let Some(reference) = credential_ref {
            self.remove_request_credentials(&reference);
        }
        Ok(changed == 1)
    }

    pub fn record_history(
        &self,
        request_id: i64,
        collection_id: i64,
        request_name: &str,
        method: &str,
        url: &str,
        status: &str,
        elapsed_ms: i64,
    ) -> Result<(), StoreError> {
        let connection = self.connect()?;
        let collection_name = connection.query_row(
            "SELECT name FROM collections WHERE id = ?1",
            [collection_id],
            |row| row.get::<_, String>(0),
        )?;
        connection.execute(
            "INSERT INTO request_history (request_id, collection_name, request_name, method, url, status, elapsed_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![request_id, collection_name, request_name, method, url, status, elapsed_ms],
        )?;
        connection.execute(
            "DELETE FROM request_history WHERE id NOT IN (SELECT id FROM request_history ORDER BY id DESC LIMIT 500)",
            [],
        )?;
        Ok(())
    }

    pub fn record_history_batch(
        &self,
        entries: &[(i64, i64, String, String, String, String, i64)],
    ) -> Result<(), StoreError> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;

        {
            let mut insert = transaction.prepare(
                "INSERT INTO request_history
                 (request_id, collection_name, request_name, method, url, status, elapsed_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;

            for (request_id, collection_id, name, method, url, status, elapsed_ms) in entries {
                let collection_name: String = transaction.query_row(
                    "SELECT name FROM collections WHERE id = ?1",
                    [collection_id],
                    |row| row.get(0),
                )?;

                insert.execute(params![
                    request_id,
                    collection_name,
                    name,
                    method,
                    url,
                    status,
                    elapsed_ms
                ])?;
            }
        }

        transaction.execute(
            "DELETE FROM request_history
             WHERE id NOT IN (
                 SELECT id FROM request_history ORDER BY id DESC LIMIT 500
             )",
            [],
        )?;

        transaction.commit()?;
        Ok(())
    }

    pub fn list_history(&self) -> Result<Vec<RequestHistory>, StoreError> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT request_id, collection_name, request_name, method, url, status, elapsed_ms, created_at FROM request_history ORDER BY id DESC LIMIT 500",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(RequestHistory {
                request_id: row.get(0)?,
                collection_name: row.get(1)?,
                request_name: row.get(2)?,
                method: row.get(3)?,
                url: row.get(4)?,
                status: row.get(5)?,
                elapsed_ms: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
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

fn row_to_stored_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSavedRequest> {
    Ok(StoredSavedRequest {
        request: SavedRequest {
            id: row.get(0)?,
            collection_id: row.get(1)?,
            name: row.get(2)?,
            method: row.get(3)?,
            url: row.get(4)?,
            headers: row.get(5)?,
            query_params: row.get(6)?,
            body: row.get(7)?,
            body_format: row.get(8)?,
            auth_type: row.get(9)?,
            auth_value: row.get(10)?,
            auth_secret: row.get(11)?,
        },
        credential_ref: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct MemoryVault(Mutex<HashMap<String, RequestCredentials>>);

    impl CredentialVault for MemoryVault {
        fn store(
            &self,
            reference: &str,
            credentials: &RequestCredentials,
        ) -> Result<(), CredentialVaultError> {
            self.0
                .lock()
                .unwrap()
                .insert(reference.to_owned(), credentials.clone());
            Ok(())
        }

        fn load(
            &self,
            reference: &str,
        ) -> Result<Option<RequestCredentials>, CredentialVaultError> {
            Ok(self.0.lock().unwrap().get(reference).cloned())
        }

        fn delete(&self, reference: &str) -> Result<(), CredentialVaultError> {
            self.0.lock().unwrap().remove(reference);
            Ok(())
        }
    }

    fn test_store(path: &Path) -> RequestStore {
        RequestStore::open_with_vault(path, std::sync::Arc::new(MemoryVault::default()))
            .expect("open store")
    }

    fn sample(collection_id: i64) -> NewRequest {
        NewRequest {
            collection_id,
            name: "Get Users".into(),
            method: "GET".into(),
            url: "https://httpbin.org/get".into(),
            headers: "Accept: application/json".into(),
            query_params: "page=1".into(),
            body: String::new(),
            body_format: "JSON".into(),
            auth_type: "None".into(),
            auth_value: String::new(),
            auth_secret: String::new(),
        }
    }

    #[test]
    fn create_read_update_delete_round_trip() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let store = test_store(&temp.path().join("test.sqlite3"));
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
            body_format: "JSON".into(),
            auth_type: "None".into(),
            auth_value: String::new(),
            auth_secret: String::new(),
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
    fn credentials_are_kept_out_of_sqlite_and_round_trip_via_vault() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("credentials.sqlite3");
        let vault = std::sync::Arc::new(MemoryVault::default());
        let store = RequestStore::open_with_vault(&path, vault.clone()).expect("open store");
        let collection_id = store.list_collections().expect("collections")[0].id;
        let request = NewRequest {
            auth_type: "Bearer".into(),
            auth_value: "top-secret-token".into(),
            ..sample(collection_id)
        };

        let saved = store.create(&request).expect("save credential request");
        assert_eq!(saved.auth_value, "top-secret-token");
        assert_eq!(
            store
                .get(saved.id)
                .expect("load request")
                .unwrap()
                .auth_value,
            "top-secret-token"
        );

        let connection = Connection::open(&path).expect("inspect database");
        let (auth_value, auth_secret, reference): (String, String, String) = connection
            .query_row(
                "SELECT auth_value, auth_secret, credential_ref FROM saved_requests WHERE id = ?1",
                [saved.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read stored fields");
        assert!(auth_value.is_empty());
        assert!(auth_secret.is_empty());
        assert!(!reference.is_empty());
        assert!(vault.0.lock().unwrap().contains_key(&reference));
    }

    #[test]
    fn migrates_legacy_plaintext_credentials_into_vault() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("legacy-credentials.sqlite3");
        let connection = Connection::open(&path).expect("open legacy database");
        connection
            .execute_batch(
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
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                auth_type TEXT NOT NULL DEFAULT 'None',
                auth_value TEXT NOT NULL DEFAULT '',
                auth_secret TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO saved_requests (name, method, url, auth_type, auth_value, auth_secret)
            VALUES ('Legacy auth', 'GET', 'https://example.com', 'Basic', 'alice', 'secret');
            "#,
            )
            .expect("create legacy schema");
        drop(connection);

        let vault = std::sync::Arc::new(MemoryVault::default());
        let store = RequestStore::open_with_vault(&path, vault.clone()).expect("migrate store");
        let collection_id = store.list_collections().expect("collections")[0].id;
        let requests = store
            .list_for_collection(collection_id)
            .expect("load migrated");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].auth_value, "alice");
        assert_eq!(requests[0].auth_secret, "secret");

        let connection = Connection::open(&path).expect("inspect migrated database");
        let (value, secret): (String, String) = connection
            .query_row(
                "SELECT auth_value, auth_secret FROM saved_requests",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read scrubbed credentials");
        assert!(value.is_empty() && secret.is_empty());
        assert!(vault.0.lock().unwrap().contains_key("legacy-request-1"));
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
        assert_eq!(initial_collections.len(), 1);
        assert_eq!(initial_collections[0].name, "Public API");
        let default = initial_collections[0].clone();
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
        let only_collection = store
            .create_collection("Only Collection")
            .expect("create final collection");
        assert!(store.delete_collection(default.id).expect("delete default"));
        assert!(matches!(
            store.delete_collection(only_collection.id),
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
