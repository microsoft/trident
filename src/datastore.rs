use anyhow::{Context, Error};
use log::info;
use std::{fs, path::Path};
use trident_api::status::HostStatus;

use crate::TRIDENT_TEMPORARY_DATASTORE_PATH;

pub(crate) struct DataStore {
    db: Option<sqlite::Connection>,
    host_status: HostStatus,
    temporary: bool,
}

impl DataStore {
    pub(crate) fn open_temporary() -> Result<Self, Error> {
        let path = Path::new(&TRIDENT_TEMPORARY_DATASTORE_PATH);
        if path.exists() {
            let existing = Self::open(path)?;
            Ok(Self {
                db: existing.db,
                host_status: existing.host_status,
                temporary: true,
            })
        } else {
            info!("Creating temporary datastore at {}", path.display());
            Ok(Self {
                db: Some(Self::make_datastore(path)?),
                host_status: HostStatus::default(),
                temporary: true,
            })
        }
    }

    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
        info!("Loading datastore from {}", path.display());
        let db = sqlite::open(path)?;
        let host_status = db
            .prepare("SELECT contents FROM hoststatus ORDER BY id DESC LIMIT 1")
            .context("Failed to create host status")?
            .into_iter()
            .next()
            .transpose()
            .context("Failed to read host status")?
            .map(|row| serde_yaml::from_str(row.read::<&str, _>(0)))
            .transpose()
            .context("Failed to parse saved host status")?
            .unwrap_or_default();

        Ok(Self {
            db: Some(db),
            host_status,
            temporary: false,
        })
    }

    pub(crate) fn is_persistent(&self) -> bool {
        !self.temporary
    }

    fn make_datastore(path: &Path) -> Result<sqlite::Connection, Error> {
        fs::create_dir_all(path.parent().unwrap())
            .context("Failed to create trident datastore directory")?;

        let db = sqlite::open(path)?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS hoststatus (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp DATETIME DEFALUT CURRENT_TIMESTAMP,
                contents TEXT NOT NULL
            )",
        )?;
        Ok(db)
    }

    pub(crate) fn persist(&mut self, path: &Path) -> Result<(), Error> {
        if self.temporary {
            let persistent_db = Self::make_datastore(path)?;
            Self::write_host_status(&persistent_db, self.host_status())?;

            self.db = Some(persistent_db);
            self.temporary = false;
        }

        Ok(())
    }

    fn write_host_status(db: &sqlite::Connection, host_status: &HostStatus) -> Result<(), Error> {
        let mut statement = db
            .prepare("INSERT INTO hoststatus (contents) VALUES (?)")
            .context("Failed to save host status (prepare)")?;
        statement
            .bind((1, &*serde_yaml::to_string(host_status)?))
            .context("Failed to save host status (bind)")?;
        statement.next().context("Failed to save host status")?;
        Ok(())
    }

    pub(crate) fn host_status(&self) -> &HostStatus {
        &self.host_status
    }

    pub(crate) fn with_host_status<T, F: FnOnce(&mut HostStatus) -> T>(
        &mut self,
        f: F,
    ) -> Result<T, Error> {
        self.try_with_host_status(|s| Ok(f(s)))
    }

    pub(crate) fn try_with_host_status<T, F: FnOnce(&mut HostStatus) -> Result<T, Error>>(
        &mut self,
        f: F,
    ) -> Result<T, Error> {
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
            self.db.as_ref().context("Datastore already closed")?,
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

#[cfg(feature = "functional-tests")]
mod functional_tests {
    #[cfg(test)]
    use anyhow::bail;
    use pytest_gen::pytest;
    #[cfg(test)]
    use tempfile::TempDir;
    #[cfg(test)]
    use trident_api::status::ReconcileState;

    #[cfg(test)]
    use super::*;

    #[pytest()]
    fn test_open_temporary_persist_reopen() {
        let _ = std::fs::remove_file(TRIDENT_TEMPORARY_DATASTORE_PATH);

        let temp_dir = TempDir::new().unwrap();
        let datastore_path = temp_dir.path().join("db.sqlite");

        // Open and initialize a temporary datastore.
        {
            let mut datastore = DataStore::open_temporary().unwrap();
            assert_eq!(
                datastore.host_status().reconcile_state,
                ReconcileState::Ready
            );
            datastore
                .with_host_status(|s| s.reconcile_state = ReconcileState::CleanInstall)
                .unwrap();
            assert_eq!(
                datastore.host_status().reconcile_state,
                ReconcileState::CleanInstall
            );
        }

        // Re-open the temporary datastore and verify that the reconcile state was retained. Then
        // persist the datastore to a new location.
        {
            let mut datastore = DataStore::open_temporary().unwrap();
            assert_eq!(
                datastore.host_status().reconcile_state,
                ReconcileState::CleanInstall
            );
            datastore.persist(&datastore_path).unwrap();
        }

        // Re-open the persisted datastore and verify that the reconcile state was retained.
        let mut datastore = DataStore::open(&datastore_path).unwrap();
        assert_eq!(
            datastore.host_status().reconcile_state,
            ReconcileState::CleanInstall
        );

        // Ensure that the datastore can be closed and re-opened.
        datastore.close();
        datastore
            .with_host_status(|s| s.reconcile_state = ReconcileState::Ready)
            .unwrap_err();

        let mut datastore = DataStore::open(&datastore_path).unwrap();
        assert_eq!(
            datastore.host_status().reconcile_state,
            ReconcileState::CleanInstall
        );

        datastore
            .try_with_host_status(|_s| -> Result<(), Error> { bail!("error") })
            .unwrap_err();
    }
}
