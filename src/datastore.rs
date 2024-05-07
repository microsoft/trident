use log::info;
use osutils::path::join_relative;
use std::{fs, path::Path};
use trident_api::{
    error::{DatastoreError, InternalError, ManagementError, ReportError, TridentError},
    status::HostStatus,
};

use crate::TRIDENT_TEMPORARY_DATASTORE_PATH;

pub struct DataStore {
    db: Option<sqlite::Connection>,
    host_status: HostStatus,
    temporary: bool,
}

impl DataStore {
    pub(crate) fn open_temporary() -> Result<Self, TridentError> {
        let path = Path::new(&TRIDENT_TEMPORARY_DATASTORE_PATH);

        if path.exists() {
            return Ok(Self {
                temporary: true,
                ..Self::open(path)?
            });
        }

        info!("Creating temporary datastore at {}", path.display());
        Ok(Self {
            db: Some(Self::make_datastore(path)?),
            host_status: HostStatus::default(),
            temporary: true,
        })
    }

    pub(crate) fn open(path: &Path) -> Result<Self, TridentError> {
        info!("Loading datastore from {}", path.display());
        let db = sqlite::open(path).structured(ManagementError::Datastore(
            DatastoreError::DatastoreLoad(path.to_owned()),
        ))?;
        let mut host_status: HostStatus = db
            .prepare("SELECT contents FROM hoststatus ORDER BY id DESC LIMIT 1")
            .structured(ManagementError::Datastore(DatastoreError::DatastoreInit))?
            .into_iter()
            .next()
            .transpose()
            .structured(ManagementError::Datastore(DatastoreError::DatastoreInit))?
            .map(|row| serde_yaml::from_str(row.read::<&str, _>(0)))
            .transpose()
            .structured(ManagementError::Datastore(
                DatastoreError::DeserializeHostStatus,
            ))?
            .unwrap_or_default();

        host_status.spec.populate_internal();

        Ok(Self {
            db: Some(db),
            host_status,
            temporary: false,
        })
    }

    /// Switches the datastore to a new path, by appending the Trident temporary datastore path to
    /// new_path.
    pub(crate) fn switch_datastore_to_path(&mut self, new_path: &Path) -> Result<(), TridentError> {
        if !self.temporary {
            return Err(TridentError::new(InternalError::Internal(
                "Attempted to switch to path {new_path} on a persistent datastore",
            )));
        }

        let db_path = join_relative(new_path, TRIDENT_TEMPORARY_DATASTORE_PATH);
        info!("Switching datastore to path {}", db_path.display());
        self.db = Some(
            sqlite::open(&db_path).structured(ManagementError::Datastore(
                DatastoreError::DatastoreLoad(db_path),
            ))?,
        );

        Ok(())
    }

    pub(crate) fn is_persistent(&self) -> bool {
        !self.temporary
    }

    fn make_datastore(path: &Path) -> Result<sqlite::Connection, TridentError> {
        fs::create_dir_all(path.parent().unwrap()).structured(ManagementError::from(
            DatastoreError::CreateDatastoreDirectory,
        ))?;

        let db =
            sqlite::open(path).structured(ManagementError::from(DatastoreError::OpenDatastore))?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS hoststatus (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp DATETIME DEFALUT CURRENT_TIMESTAMP,
                contents TEXT NOT NULL
            )",
        )
        .structured(ManagementError::from(DatastoreError::DatastoreInit))?;
        Ok(db)
    }

    pub(crate) fn persist(&mut self, path: &Path) -> Result<(), TridentError> {
        if self.temporary {
            let persistent_db = Self::make_datastore(path)?;
            Self::write_host_status(&persistent_db, self.host_status())?;

            self.db = Some(persistent_db);
            self.temporary = false;
        }

        Ok(())
    }

    fn write_host_status(
        db: &sqlite::Connection,
        host_status: &HostStatus,
    ) -> Result<(), TridentError> {
        let mut statement = db
            .prepare("INSERT INTO hoststatus (contents) VALUES (?)")
            .structured(ManagementError::from(DatastoreError::DatastoreWrite))?;
        statement
            .bind((
                1,
                &*serde_yaml::to_string(host_status)
                    .structured(ManagementError::from(DatastoreError::SerializeHostStatus))?,
            ))
            .structured(ManagementError::from(DatastoreError::DatastoreWrite))?;
        statement
            .next()
            .structured(ManagementError::from(DatastoreError::DatastoreWrite))?;

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

        // Call the provided method and return early if the host status was not modified.
        let ret = f(&mut updated);
        if &updated == self.host_status() {
            return ret;
        }

        self.host_status = updated;

        // Always attempt to save the updated host status, even if the previous call failed,
        // but only report errors from saving the host status if it succeeded.
        let ret2 = Self::write_host_status(
            self.db
                .as_ref()
                .structured(ManagementError::from(DatastoreError::DatastoreClosed))?,
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
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;
    use tempfile::TempDir;
    use trident_api::error::InternalError;
    use trident_api::status::{ServicingState, ServicingType};

    #[functional_test]
    fn test_open_temporary_persist_reopen() {
        let _ = std::fs::remove_file(TRIDENT_TEMPORARY_DATASTORE_PATH);

        let temp_dir = TempDir::new().unwrap();
        let datastore_path = temp_dir.path().join("db.sqlite");

        // Open and initialize a temporary datastore.
        {
            let mut datastore = DataStore::open_temporary().unwrap();
            assert_eq!(datastore.host_status().servicing_type, None);
            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::NotProvisioned
            );

            // Update servicing type and state for clean install
            datastore
                .with_host_status(|s| s.servicing_type = Some(ServicingType::CleanInstall))
                .unwrap();
            datastore
                .with_host_status(|s| s.servicing_state = ServicingState::StagingDeployment)
                .unwrap();

            assert_eq!(
                datastore.host_status().servicing_type,
                Some(ServicingType::CleanInstall)
            );
            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::StagingDeployment
            );
        }

        // Re-open the temporary datastore and verify that the servicing type and state were
        // retained. Then re-rewrite and persist the datastore to a new location.
        {
            let mut datastore = DataStore::open_temporary().unwrap();
            assert_eq!(
                datastore.host_status().servicing_type,
                Some(ServicingType::CleanInstall)
            );
            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::StagingDeployment
            );

            datastore
                .with_host_status(|s| s.boot_next = Some("test".to_string()))
                .unwrap();
            datastore.persist(&datastore_path).unwrap();
        }

        // Re-open the persisted datastore and verify that the servicing state was retained.
        let mut datastore = DataStore::open(&datastore_path).unwrap();
        assert_eq!(datastore.host_status().boot_next.as_deref(), Some("test"));

        // Ensure that the datastore can be closed and re-opened.
        datastore.close();
        datastore
            .with_host_status(|s| s.servicing_state = ServicingState::Provisioned)
            .unwrap_err();

        let mut datastore = DataStore::open(&datastore_path).unwrap();
        assert_eq!(
            datastore.host_status().servicing_state,
            ServicingState::StagingDeployment
        );

        datastore
            .try_with_host_status(|_s| -> Result<(), TridentError> {
                Err(TridentError::new(InternalError::Internal("error")))
            })
            .unwrap_err();
    }
}
