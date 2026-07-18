//! SQLite-backed persistent storage for credentials and state.
//!
//! Stores:
//! - iLink bot credentials (token, base_url, account_id)

use crate::error::{GatewayError, Result};
use rusqlite::Connection;

/// Saved iLink bot credentials.
#[derive(Debug, Clone)]
pub struct StoredCredentials {
    pub account_id: String,
    pub token: String,
    pub base_url: String,
    #[allow(dead_code)]
    pub user_id: String,
    #[allow(dead_code)]
    pub saved_at: String,
}

/// SQLite-backed credential store.
pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Open or create the database at the given path.
    ///
    /// Creates the `credentials` table on first access.
    pub fn new(path: &str) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| GatewayError::Storage(format!("Failed to create database directory: {e}")))?;
            }
        }
        let conn = Connection::open(path)
            .map_err(|e| GatewayError::Storage(format!("Failed to open database: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS credentials (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id  TEXT NOT NULL,
                token       TEXT NOT NULL,
                base_url    TEXT NOT NULL,
                user_id     TEXT NOT NULL,
                saved_at    TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .map_err(|e| GatewayError::Storage(format!("Failed to create schema: {e}")))?;
        Ok(SqliteStore { conn })
    }

    /// Save iLink bot credentials, replacing any existing row.
    pub fn save_credentials(
        &self,
        account_id: &str,
        token: &str,
        base_url: &str,
        user_id: &str,
    ) -> Result<()> {
        // Delete any existing row so there is only ever one set of credentials.
        self.conn
            .execute("DELETE FROM credentials", [])
            .map_err(|e| GatewayError::Storage(format!("Failed to clear credentials: {e}")))?;

        self.conn
            .execute(
                "INSERT INTO credentials (account_id, token, base_url, user_id) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![account_id, token, base_url, user_id],
            )
            .map_err(|e| GatewayError::Storage(format!("Failed to save credentials: {e}")))?;
        Ok(())
    }

    /// Load the most recently saved credentials, if any.
    pub fn load_credentials(&self) -> Result<Option<StoredCredentials>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT account_id, token, base_url, user_id, saved_at \
                 FROM credentials ORDER BY id DESC LIMIT 1",
            )
            .map_err(|e| GatewayError::Storage(format!("Failed to prepare query: {e}")))?;

        match stmt.query_row([], |row| {
            Ok(StoredCredentials {
                account_id: row.get(0)?,
                token: row.get(1)?,
                base_url: row.get(2)?,
                user_id: row.get(3)?,
                saved_at: row.get(4)?,
            })
        }) {
            Ok(creds) => Ok(Some(creds)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(GatewayError::Storage(format!(
                "Failed to load credentials: {e}"
            ))),
        }
    }

    /// Delete all stored credentials.
    #[allow(dead_code)]
    pub fn delete_credentials(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM credentials", [])
            .map_err(|e| GatewayError::Storage(format!("Failed to delete credentials: {e}")))?;
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_save_and_load_credentials() {
        let file = NamedTempFile::new().unwrap();
        let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();

        store
            .save_credentials("acct-1", "tok-abc", "https://ilink.example.com", "user@wx")
            .unwrap();
        let creds = store.load_credentials().unwrap().unwrap();

        assert_eq!(creds.account_id, "acct-1");
        assert_eq!(creds.token, "tok-abc");
        assert_eq!(creds.base_url, "https://ilink.example.com");
        assert_eq!(creds.user_id, "user@wx");
        assert!(!creds.saved_at.is_empty(), "saved_at should be populated");
    }

    #[test]
    fn test_overwrite_existing_credentials() {
        let file = NamedTempFile::new().unwrap();
        let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();

        store
            .save_credentials("old-acct", "old-tok", "old-url", "old@wx")
            .unwrap();
        store
            .save_credentials("new-acct", "new-tok", "new-url", "new@wx")
            .unwrap();

        let creds = store.load_credentials().unwrap().unwrap();
        assert_eq!(creds.account_id, "new-acct");
        assert_eq!(creds.token, "new-tok");
        assert_eq!(creds.base_url, "new-url");
        assert_eq!(creds.user_id, "new@wx");
    }

    #[test]
    fn test_delete_credentials() {
        let file = NamedTempFile::new().unwrap();
        let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();

        store
            .save_credentials("acct-1", "tok-1", "url-1", "u@wx")
            .unwrap();
        store.delete_credentials().unwrap();

        assert!(store.load_credentials().unwrap().is_none());
    }

    #[test]
    fn test_empty_database_returns_none() {
        let file = NamedTempFile::new().unwrap();
        let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();

        let creds = store.load_credentials().unwrap();
        assert!(creds.is_none());
    }

    #[test]
    fn test_schema_created_on_first_access() {
        let file = NamedTempFile::new().unwrap();
        // Creating the store should execute CREATE TABLE IF NOT EXISTS.
        SqliteStore::new(file.path().to_str().unwrap()).unwrap();

        // Re-open the same file with a raw connection and verify the table exists.
        let conn = Connection::open(file.path()).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='credentials'")
            .unwrap();
        let table_name: String = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(table_name, "credentials");
    }

    #[test]
    fn test_load_credentials_after_reopen() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        // Write credentials then drop the store.
        {
            let store = SqliteStore::new(&path).unwrap();
            store
                .save_credentials("acct-x", "tok-x", "url-x", "u@x")
                .unwrap();
        }

        // Re-open and verify they are still there.
        let store = SqliteStore::new(&path).unwrap();
        let creds = store.load_credentials().unwrap().unwrap();
        assert_eq!(creds.account_id, "acct-x");
        assert_eq!(creds.token, "tok-x");
    }
}
