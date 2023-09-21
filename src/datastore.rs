use anyhow::{Context, Error};
use std::path::Path;
use trident_api::status::HostStatus;

pub(crate) struct DataStore {
    db: sqlite::Connection,
    host_status: HostStatus,
}

impl DataStore {
    pub(crate) fn create(path: &Path, host_status: HostStatus) -> Result<Self, Error> {
        let db = sqlite::open(path)?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS hoststatus (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp DATETIME DEFALUT CURRENT_TIMESTAMP,
                contents TEXT NOT NULL
            )",
        )?;
        Self::write_host_status(&db, &host_status)?;
        Ok(Self { db, host_status })
    }

    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
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

        Ok(Self { db, host_status })
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

    pub(crate) fn with_host_status<T, F: FnOnce(&mut HostStatus) -> Result<T, Error>>(
        &mut self,
        f: F,
    ) -> Result<T, Error> {
        let mut updated = self.host_status.clone();

        // Call the provided method and return early if the host status was not modified.
        let ret = f(&mut updated);
        if updated == self.host_status {
            return ret;
        }
        self.host_status = updated;

        // Always attempt to save the updated host status, even if the previous call failed, but
        // only report errors from saving the host status if it succeeded.
        let ret2 = Self::write_host_status(&self.db, &self.host_status);
        if ret.is_ok() {
            ret2?;
        }

        ret
    }
}
