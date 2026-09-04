use std::{fs, path::Path};

use log::{debug, warn};
use serde::{de::DeserializeOwned, Serialize};
use sqlite::State;
use uuid::Uuid;

use trident_api::{
    error::{
        DatastoreError, InternalError, ReportError, ServicingError, TridentError, TridentResultExt,
    },
    status::{decode_host_status, HostStatus, TridentVersion},
};

use crate::TRIDENT_SEMVER_VERSION;

/// Key under which the datastore's unique correlation ID is stored in the
/// generic key-value table. This ID is generated once (on first access) and
/// persisted for the lifetime of the datastore. It is intended to be added to
/// tracing/telemetry so that all activity for a given host installation can
/// be correlated.
const CORRELATION_ID_KEY: &str = "correlation-id";

pub struct DataStore {
    db: Option<sqlite::Connection>,
    host_status: HostStatus,
    temporary: bool,
}

impl DataStore {
    pub fn open_or_create(path: &Path) -> Result<Self, TridentError> {
        if path.exists() {
            return Self::open(path);
        }

        debug!("Creating temporary datastore at {}", path.display());
        Ok(Self {
            db: Some(Self::make_datastore(path)?),
            host_status: HostStatus {
                is_management_os: true,
                ..Default::default()
            },
            temporary: true,
        })
    }

    pub(crate) fn open(path: &Path) -> Result<Self, TridentError> {
        debug!("Loading datastore from {}", path.display());
        let mut db = sqlite::open(path).structured(ServicingError::Datastore {
            inner: DatastoreError::LoadDatastore {
                path: path.to_string_lossy().into(),
            },
        })?;
        // Multiple connections to the same datastore file (e.g. concurrent
        // daemon RPC handlers) can briefly contend for the write lock; wait
        // for it rather than failing immediately with "database is locked".
        Self::set_busy_timeout(&mut db)?;
        // Existing datastores may predate a table added in a later Trident
        // version (e.g. `keyvalue`). Idempotently ensure the full schema is
        // present so upgraded hosts don't fail with "no such table" the
        // first time a new table is accessed.
        Self::ensure_schema(&db)?;
        let host_status_yaml: Option<serde_yaml::Value> = db
            .prepare("SELECT contents FROM hoststatus ORDER BY id DESC LIMIT 1")
            .structured(ServicingError::Datastore {
                inner: DatastoreError::InitializeDatastore,
            })?
            .into_iter()
            .next()
            .transpose()
            .structured(ServicingError::Datastore {
                inner: DatastoreError::InitializeDatastore,
            })?
            .map(|row| serde_yaml::from_str(row.read::<&str, _>(0)))
            .transpose()
            .structured(ServicingError::Datastore {
                inner: DatastoreError::InitializeDatastore,
            })
            .message("Failed to parse Host Status as YAML")?;

        let host_status = host_status_yaml
            .map(decode_host_status)
            .transpose()
            .structured(ServicingError::Datastore {
                inner: DatastoreError::InitializeDatastore,
            })?
            .unwrap_or(HostStatus {
                is_management_os: true,
                ..Default::default()
            });

        Ok(Self {
            db: Some(db),
            temporary: host_status.is_management_os,
            host_status,
        })
    }

    /// Retrieve all HostStatus entries from the datastore, sorted from newest to oldest.
    pub(crate) fn get_host_statuses(&self) -> Result<Vec<Option<HostStatus>>, TridentError> {
        let mut all_rows_data: Vec<Option<HostStatus>> = Vec::new();

        // Read all HostStatus entries from the datastore, parse them into
        // HostStatus structs, and return a slice of them.
        let mut query_statement = self
            .db
            .as_ref()
            .structured(ServicingError::from(DatastoreError::OpenDatastore))?
            .prepare("SELECT contents FROM hoststatus ORDER BY id DESC")
            .structured(ServicingError::Datastore {
                inner: DatastoreError::ReadDatastore,
            })
            .message("Failed to read all database host statuses")?;

        loop {
            match query_statement.next() {
                Ok(State::Done) => break,
                Err(e) => {
                    warn!("Failed to get next datastore row: {:?}", e);
                    all_rows_data.push(None);
                    break;
                }
                Ok(State::Row) => {} // continue below
            }
            all_rows_data.push(self.parse_host_status(query_statement.read::<String, _>(0)));
        }
        Ok(all_rows_data)
    }

    pub(crate) fn is_persistent(&self) -> bool {
        !self.temporary
    }

    fn make_datastore(path: &Path) -> Result<sqlite::Connection, TridentError> {
        fs::create_dir_all(path.parent().unwrap()).structured(ServicingError::from(
            DatastoreError::CreateDatastoreDirectory,
        ))?;

        let mut db =
            sqlite::open(path).structured(ServicingError::from(DatastoreError::OpenDatastore))?;
        Self::set_busy_timeout(&mut db)?;
        Self::ensure_schema(&db)?;
        Ok(db)
    }

    /// Wait (rather than immediately failing with "database is locked") for
    /// up to five seconds when another connection holds the write lock on
    /// this datastore file. Needed because more than one connection to the
    /// same datastore can legitimately exist at once (e.g. concurrent
    /// daemon RPC handlers each calling `Trident::new`), and SQLite's
    /// default busy timeout is zero.
    fn set_busy_timeout(db: &mut sqlite::Connection) -> Result<(), TridentError> {
        db.set_busy_timeout(5000)
            .structured(ServicingError::from(DatastoreError::OpenDatastore))
    }

    /// Idempotently create any tables that don't already exist. Safe to call
    /// on both newly-created and pre-existing datastores, so that a
    /// datastore created by an older Trident version picks up tables added
    /// by a newer version the next time it is opened.
    fn ensure_schema(db: &sqlite::Connection) -> Result<(), TridentError> {
        db.execute(
            "CREATE TABLE IF NOT EXISTS hoststatus (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
                contents TEXT NOT NULL
            )",
        )
        .structured(ServicingError::from(DatastoreError::InitializeDatastore))?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS keyvalue (
                key TEXT PRIMARY KEY,
                contents TEXT NOT NULL
            )",
        )
        .structured(ServicingError::from(DatastoreError::InitializeDatastore))?;
        Ok(())
    }

    pub(crate) fn persist(&mut self, path: &Path) -> Result<(), TridentError> {
        if self.temporary {
            let persistent_db = Self::make_datastore(path)?;
            self.host_status.is_management_os = false;
            self.host_status.trident_version =
                TridentVersion::SemVer(TRIDENT_SEMVER_VERSION.clone());
            Self::write_host_status(&persistent_db, self.host_status())?;

            // Carry over any generic key-value entries (e.g. the correlation ID)
            // recorded in the temporary datastore into the persistent one, so
            // they survive the transition from temporary to persistent
            // storage.
            if let Some(temporary_db) = self.db.as_ref() {
                Self::copy_key_values(temporary_db, &persistent_db)?;
            }

            self.db = Some(persistent_db);
            self.temporary = false;
        }

        Ok(())
    }

    /// Copy all rows of the generic key-value table from `source` into
    /// `destination`, overwriting any conflicting keys already present in
    /// `destination`.
    ///
    /// `source` and `destination` may be two live connections to the *same*
    /// underlying SQLite file (e.g. an offline `persist` whose destination
    /// path is the currently-open datastore's own path). SQLite's locking is
    /// per-connection, so a `SELECT` left active on `source` holds a read
    /// lock that a write from `destination` on the same file would have to
    /// wait on -- and since both connections are driven from this single
    /// thread, that wait can never be satisfied ("database is locked").
    /// To avoid this, fully read and finalize the source query *before*
    /// issuing any writes to `destination`.
    fn copy_key_values(
        source: &sqlite::Connection,
        destination: &sqlite::Connection,
    ) -> Result<(), TridentError> {
        let mut rows: Vec<(String, String)> = Vec::new();
        {
            let mut query_statement = source
                .prepare("SELECT key, contents FROM keyvalue")
                .structured(ServicingError::from(DatastoreError::ReadDatastore))?;

            loop {
                match query_statement.next() {
                    Ok(State::Done) => break,
                    Err(e) => {
                        return Err(e)
                            .structured(ServicingError::from(DatastoreError::ReadDatastore))
                            .message("Failed to get next keyvalue row while copying datastore");
                    }
                    Ok(State::Row) => {} // continue below
                }

                let key = query_statement
                    .read::<String, _>(0)
                    .structured(ServicingError::from(DatastoreError::ReadDatastore))?;
                let contents = query_statement
                    .read::<String, _>(1)
                    .structured(ServicingError::from(DatastoreError::ReadDatastore))?;

                rows.push((key, contents));
            }
            // `query_statement` is dropped here, finalizing the source
            // SELECT and releasing its read lock before any writes below.
        }

        for (key, contents) in rows {
            let mut insert_statement = destination
                .prepare(
                    "INSERT INTO keyvalue (key, contents) VALUES (?, ?) \
                     ON CONFLICT(key) DO UPDATE SET contents = excluded.contents",
                )
                .structured(ServicingError::Datastore {
                    inner: DatastoreError::WriteKeyValue { key: key.clone() },
                })?;
            insert_statement
                .bind((1, &*key))
                .structured(ServicingError::Datastore {
                    inner: DatastoreError::WriteKeyValue { key: key.clone() },
                })?;
            insert_statement
                .bind((2, &*contents))
                .structured(ServicingError::Datastore {
                    inner: DatastoreError::WriteKeyValue { key: key.clone() },
                })?;
            insert_statement
                .next()
                .structured(ServicingError::Datastore {
                    inner: DatastoreError::WriteKeyValue { key },
                })?;
        }

        Ok(())
    }

    fn write_host_status(
        db: &sqlite::Connection,
        host_status: &HostStatus,
    ) -> Result<(), TridentError> {
        // Create a mutable copy of the Host Status to add Trident version before writing.
        let mut host_status_with_trident_version = host_status.clone();
        host_status_with_trident_version.trident_version =
            TridentVersion::SemVer(TRIDENT_SEMVER_VERSION.clone());
        let mut statement = db
            .prepare("INSERT INTO hoststatus (contents) VALUES (?)")
            .structured(ServicingError::from(DatastoreError::WriteToDatastore))?;
        statement
            .bind((
                1,
                &*serde_yaml::to_string(&host_status_with_trident_version)
                    .structured(InternalError::SerializeHostStatus)?,
            ))
            .structured(ServicingError::from(DatastoreError::WriteToDatastore))?;
        statement
            .next()
            .structured(ServicingError::from(DatastoreError::WriteToDatastore))?;

        Ok(())
    }

    pub(crate) fn host_status(&self) -> &HostStatus {
        &self.host_status
    }

    pub(crate) fn with_host_status<T, F: FnOnce(&mut HostStatus) -> T>(
        &mut self,
        f: F,
    ) -> Result<T, TridentError> {
        self.try_with_host_status(|s| Ok(f(s)))
    }

    pub(crate) fn try_with_host_status<T, F: FnOnce(&mut HostStatus) -> Result<T, TridentError>>(
        &mut self,
        f: F,
    ) -> Result<T, TridentError> {
        let mut updated = self.host_status().clone();

        // Call the provided method and return early if the Host Status was not modified.
        let ret = f(&mut updated);
        if &updated == self.host_status() {
            return ret;
        }

        self.host_status = updated;

        // Always attempt to save the updated Host Status, even if the previous call failed,
        // but only report errors from saving the Host Status if it succeeded.
        let ret2 = Self::write_host_status(
            self.db
                .as_ref()
                .structured(ServicingError::from(DatastoreError::WriteToClosedDatastore))?,
            &self.host_status,
        );
        if ret.is_ok() {
            ret2?;
        }

        ret
    }

    /// Close the connection to the datastore.
    ///
    /// This is necessary before unmounting the partition containing this datastore, but will cause
    /// any further attempts to use the datastore to fail.
    pub(crate) fn close(&mut self) {
        self.db = None;
    }

    /// Retrieve a structured value stored under `key` in the datastore's
    /// generic key-value table, if present.
    ///
    /// Values are serialized as JSON, so any type implementing
    /// `serde::Serialize`/`serde::de::DeserializeOwned` can be stored, not
    /// just `HostStatus`.
    pub(crate) fn get_value<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, TridentError> {
        let db = self
            .db
            .as_ref()
            .structured(ServicingError::from(DatastoreError::OpenDatastore))?;

        let mut statement = db
            .prepare("SELECT contents FROM keyvalue WHERE key = ?")
            .structured(ServicingError::Datastore {
                inner: DatastoreError::ReadKeyValue {
                    key: key.to_string(),
                },
            })?;
        statement
            .bind((1, key))
            .structured(ServicingError::Datastore {
                inner: DatastoreError::ReadKeyValue {
                    key: key.to_string(),
                },
            })?;

        match statement.next().structured(ServicingError::Datastore {
            inner: DatastoreError::ReadKeyValue {
                key: key.to_string(),
            },
        })? {
            State::Row => {
                let contents =
                    statement
                        .read::<String, _>(0)
                        .structured(ServicingError::Datastore {
                            inner: DatastoreError::ReadKeyValue {
                                key: key.to_string(),
                            },
                        })?;
                let value = serde_json::from_str(&contents).structured(
                    InternalError::DeserializeValue {
                        key: key.to_string(),
                    },
                )?;
                Ok(Some(value))
            }
            State::Done => Ok(None),
        }
    }

    /// Store a structured value under `key` in the datastore's generic
    /// key-value table, overwriting any previous value stored under the
    /// same key.
    ///
    /// Values are serialized as JSON, so any type implementing
    /// `serde::Serialize`/`serde::de::DeserializeOwned` can be stored, not
    /// just `HostStatus`.
    ///
    /// `correlation_id` is currently the only first-party caller of the
    /// generic key-value store, and it needs insert-if-absent semantics
    /// (see `set_value_if_absent`) rather than an unconditional overwrite,
    /// so this unconditional-overwrite variant is presently exercised only
    /// by tests. It's kept as part of the generic key-value API (see
    /// `get_value`) for future callers that do want overwrite semantics.
    #[allow(dead_code)]
    pub(crate) fn set_value<T: Serialize>(&self, key: &str, value: &T) -> Result<(), TridentError> {
        let contents = serde_json::to_string(value).structured(InternalError::SerializeValue {
            key: key.to_string(),
        })?;
        self.write_key_value_row(
            key,
            &contents,
            "INSERT INTO keyvalue (key, contents) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET contents = excluded.contents",
        )
    }

    /// Like [`Self::set_value`], but only inserts a row if `key` does not
    /// already have one; an existing row is left untouched. Used where two
    /// datastore connections could race to perform "first access"
    /// initialization of a key (see [`Self::correlation_id`]): whichever
    /// connection's insert commits first wins, and the other's insert
    /// becomes a no-op instead of overwriting the winner's value.
    pub(crate) fn set_value_if_absent<T: Serialize>(
        &self,
        key: &str,
        value: &T,
    ) -> Result<(), TridentError> {
        let contents = serde_json::to_string(value).structured(InternalError::SerializeValue {
            key: key.to_string(),
        })?;
        self.write_key_value_row(
            key,
            &contents,
            "INSERT INTO keyvalue (key, contents) VALUES (?, ?) \
             ON CONFLICT(key) DO NOTHING",
        )
    }

    /// Execute a parameterized `INSERT ... ON CONFLICT ...` against the
    /// `keyvalue` table, binding `key` and `contents` as the two `?`
    /// placeholders in `sql`.
    fn write_key_value_row(
        &self,
        key: &str,
        contents: &str,
        sql: &str,
    ) -> Result<(), TridentError> {
        let db = self
            .db
            .as_ref()
            .structured(ServicingError::from(DatastoreError::WriteToClosedDatastore))?;

        let mut statement = db.prepare(sql).structured(ServicingError::Datastore {
            inner: DatastoreError::WriteKeyValue {
                key: key.to_string(),
            },
        })?;
        statement
            .bind((1, key))
            .structured(ServicingError::Datastore {
                inner: DatastoreError::WriteKeyValue {
                    key: key.to_string(),
                },
            })?;
        statement
            .bind((2, contents))
            .structured(ServicingError::Datastore {
                inner: DatastoreError::WriteKeyValue {
                    key: key.to_string(),
                },
            })?;
        statement.next().structured(ServicingError::Datastore {
            inner: DatastoreError::WriteKeyValue {
                key: key.to_string(),
            },
        })?;

        Ok(())
    }

    /// Retrieve this datastore's unique correlation ID, generating and
    /// persisting a new one on first access.
    ///
    /// This ID is stable for the lifetime of the datastore (surviving the
    /// temporary-to-persistent transition performed by `persist`), and is
    /// intended to be attached to tracing/telemetry so that activity for a
    /// given host installation can be correlated across logs and traces.
    ///
    /// First access is not a simple read-then-write: `Trident::new` may be
    /// invoked concurrently (e.g. by multiple daemon RPC handlers), each
    /// opening its own connection to the same datastore file. A naive
    /// "read, and if absent generate + write" would let two connections
    /// both observe no row, generate different UUIDs, and each overwrite
    /// the other -- leaving one caller tracing with an ID that was never
    /// actually persisted. Instead, unconditionally attempt to insert a
    /// freshly generated ID with `ON CONFLICT DO NOTHING` (a no-op if
    /// another connection already inserted one first), then read back
    /// whichever ID actually won that race.
    pub fn correlation_id(&mut self) -> Result<Uuid, TridentError> {
        if let Some(id) = self.get_value::<Uuid>(CORRELATION_ID_KEY)? {
            return Ok(id);
        }

        self.set_value_if_absent(CORRELATION_ID_KEY, &Uuid::new_v4())?;

        self.get_value::<Uuid>(CORRELATION_ID_KEY)?
            .structured(InternalError::Internal(
                "Correlation ID missing immediately after being inserted",
            ))
    }

    /// Parse a single HostStatus entry from a datastore query result.
    /// 1. Read each row as a string containing YAML-encoded Host Status.
    /// 2. Decode the YAML string into a serde_yaml Value.
    /// 3. Use decode_host_status to convert the serde_yaml Value into a HostStatus struct.
    ///
    /// If any step fails, log the error and push None for that row.
    /// If all steps succeed, push Some(HostStatus) for that row.
    fn parse_host_status(&self, query_result: Result<String, sqlite::Error>) -> Option<HostStatus> {
        let host_status_yaml = match query_result {
            Ok(yaml) => yaml,
            Err(e) => {
                warn!("Failed to read datastore row: {:?}", e);
                return None;
            }
        };

        let host_status_value: serde_yaml::Value = match serde_yaml::from_str(&host_status_yaml) {
            Ok(host_status_value) => host_status_value,
            Err(e) => {
                warn!("Failed to parse Host Status as serde value: {:?}", e);
                return None;
            }
        };

        match decode_host_status(host_status_value) {
            Ok(host_status) => Some(host_status),
            Err(e) => {
                warn!("Failed to parse Host Status: {:?}", e);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_make_datastore() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("db.sqlite");

        // Create datastore
        let _ = super::DataStore::make_datastore(&path).unwrap();
        assert!(path.exists());

        // Reopen datastore
        let _ = super::DataStore::make_datastore(&path).unwrap();
        assert!(path.exists());

        // Create datastore in a subdirectory
        let new_path = temp_dir.path().join("new").join("db.sqlite");
        let _ = super::DataStore::make_datastore(&new_path).unwrap();
        assert!(new_path.exists());

        temp_dir.close().unwrap();
    }

    #[test]
    fn test_parse_host_status() {
        let ds = super::DataStore {
            db: None,
            host_status: Default::default(),
            temporary: true,
        };
        // Validate when db row is an error
        assert!(ds
            .parse_host_status(Err(sqlite::Error {
                code: None,
                message: None,
            }))
            .is_none());
        // Validate when db row cannot be parsed into serde_yaml::Value
        assert!(ds
            .parse_host_status(Ok("[@ notserdevalue".to_string()))
            .is_none());
        // Validate when db row can be parsed into serde_yaml::Value but not HostStatus
        assert!(ds
            .parse_host_status(Ok("serdeyaml: but-not-host-status".to_string()))
            .is_none());
        // Validate when db row can be parsed into serde_yaml::Value and HostStatus
        let valid_host_status = super::HostStatus {
            ..Default::default()
        };
        assert!(ds
            .parse_host_status(Ok(serde_yaml::to_string(&valid_host_status).unwrap()))
            .is_some());
    }

    #[test]
    fn test_generic_key_value_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("db.sqlite");
        let db = super::DataStore::make_datastore(&path).unwrap();
        let datastore = super::DataStore {
            db: Some(db),
            host_status: Default::default(),
            temporary: false,
        };

        // No value stored yet.
        assert_eq!(datastore.get_value::<String>("some-key").unwrap(), None);

        // Store and retrieve a value.
        datastore
            .set_value("some-key", &"some-value".to_string())
            .unwrap();
        assert_eq!(
            datastore.get_value::<String>("some-key").unwrap(),
            Some("some-value".to_string())
        );

        // Overwrite the value.
        datastore
            .set_value("some-key", &"other-value".to_string())
            .unwrap();
        assert_eq!(
            datastore.get_value::<String>("some-key").unwrap(),
            Some("other-value".to_string())
        );

        temp_dir.close().unwrap();
    }

    #[test]
    /// Regression test: a datastore created by an older Trident version that
    /// predates the `keyvalue` table (only `hoststatus` exists) must still
    /// be usable after `open()` -- in particular, `correlation_id()` must
    /// not fail with "no such table: keyvalue".
    fn test_open_upgrades_pre_existing_datastore_schema() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("db.sqlite");

        // Simulate a datastore created before the `keyvalue` table existed:
        // create only the `hoststatus` table.
        {
            let db = sqlite::open(&path).unwrap();
            db.execute(
                "CREATE TABLE IF NOT EXISTS hoststatus (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
                    contents TEXT NOT NULL
                )",
            )
            .unwrap();
        }

        let mut datastore = super::DataStore::open(&path).unwrap();
        // Should not fail with "no such table: keyvalue".
        datastore.correlation_id().unwrap();

        temp_dir.close().unwrap();
    }

    #[test]
    fn test_correlation_id_concurrent_first_access_is_consistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("db.sqlite");

        // Create the datastore (and its schema) up front, then open two
        // separate connections to it, simulating two daemon RPC handlers
        // concurrently calling `Trident::new` (and therefore
        // `correlation_id`) against the same datastore path.
        super::DataStore::make_datastore(&path).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut datastore = super::DataStore::open(&path).unwrap();
                    // Synchronize so both threads attempt "first access"
                    // (no correlation ID persisted yet) as close together
                    // as possible.
                    barrier.wait();
                    datastore.correlation_id().unwrap()
                })
            })
            .collect();

        let ids: Vec<uuid::Uuid> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            ids[0], ids[1],
            "concurrent first access returned inconsistent correlation IDs"
        );

        temp_dir.close().unwrap();
    }

    #[test]
    fn test_correlation_id_is_stable() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("db.sqlite");
        let db = super::DataStore::make_datastore(&path).unwrap();
        let mut datastore = super::DataStore {
            db: Some(db),
            host_status: Default::default(),
            temporary: false,
        };

        let id = datastore.correlation_id().unwrap();
        // Calling correlation_id again should return the same ID, not generate a
        // new one.
        assert_eq!(datastore.correlation_id().unwrap(), id);

        temp_dir.close().unwrap();
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use tempfile::TempDir;

    use pytest_gen::functional_test;
    use trident_api::{error::ErrorKind, status::ServicingState};

    #[functional_test]
    fn test_open_temporary_persist_reopen() {
        let temp_dir = TempDir::new().unwrap();
        let datastore_temp_path = temp_dir.path().join("db-tmp.sqlite");
        let datastore_path = temp_dir.path().join("db.sqlite");

        // Open and initialize a temporary datastore.
        {
            let mut datastore = DataStore::open_or_create(&datastore_temp_path).unwrap();
            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::NotProvisioned
            );

            // Update Host Status contents.
            datastore
                .with_host_status(|s| s.servicing_state = ServicingState::AbUpdateStaged)
                .unwrap();

            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::AbUpdateStaged
            );
        }

        // Re-open the temporary datastore and verify that the servicing state was retained. Then
        // re-rewrite and persist the datastore to a new location.
        {
            let mut datastore = DataStore::open_or_create(&datastore_temp_path).unwrap();
            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::AbUpdateStaged
            );
            datastore
                .with_host_status(|s| s.servicing_state = ServicingState::Provisioned)
                .unwrap();
            datastore.persist(&datastore_path).unwrap();
        }

        // Re-open the persisted datastore and verify that the servicing state was retained.
        let mut datastore = DataStore::open(&datastore_path).unwrap();
        assert_eq!(
            datastore.host_status().servicing_state,
            ServicingState::Provisioned
        );
        // Ensure that the datastore can be closed and re-opened.
        datastore.close();
        assert_eq!(
            datastore
                .with_host_status(|s| s.servicing_state = ServicingState::AbUpdateStaged)
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::Datastore {
                inner: DatastoreError::WriteToClosedDatastore
            })
        );

        let datastore = DataStore::open(&datastore_path).unwrap();
        assert_eq!(
            datastore.host_status().servicing_state,
            ServicingState::Provisioned
        );
    }

    #[functional_test]
    /// Regression test: `persist` supports a destination path equal to the
    /// currently-open (temporary) datastore's own path -- the offline
    /// provisioning flow does this. Before the fix, `copy_key_values` kept
    /// the source `SELECT` active while writing to the destination
    /// connection, and since both connections point at the same file, the
    /// destination write would block on the source's read lock forever
    /// (mitigated only by the busy timeout, so this would previously fail
    /// with "database is locked" rather than deadlock outright).
    fn test_persist_to_same_path_does_not_deadlock() {
        let temp_dir = TempDir::new().unwrap();
        let datastore_path = temp_dir.path().join("db.sqlite");

        let mut datastore = DataStore::open_or_create(&datastore_path).unwrap();
        let correlation_id = datastore.correlation_id().unwrap();

        // Persist to the exact same path the datastore is currently open at.
        datastore.persist(&datastore_path).unwrap();

        assert_eq!(
            datastore.correlation_id().unwrap(),
            correlation_id,
            "Correlation ID should survive a self-persist"
        );
    }

    #[functional_test]
    fn test_correlation_id_survives_persist() {
        let temp_dir = TempDir::new().unwrap();
        let datastore_temp_path = temp_dir.path().join("db-tmp.sqlite");
        let datastore_path = temp_dir.path().join("db.sqlite");

        // Generate a correlation ID in the temporary datastore, then persist it.
        let correlation_id = {
            let mut datastore = DataStore::open_or_create(&datastore_temp_path).unwrap();
            let correlation_id = datastore.correlation_id().unwrap();
            datastore.persist(&datastore_path).unwrap();
            correlation_id
        };

        // Re-open the persisted datastore and verify the same correlation ID is
        // returned, rather than a new one being generated.
        let mut datastore = DataStore::open(&datastore_path).unwrap();
        assert_eq!(datastore.correlation_id().unwrap(), correlation_id);
    }
}
