use anyhow::{bail, Context, Error};
use std::{fs, mem, path::Path};
use trident_api::status::HostStatus;

pub(crate) enum DataStore {
    Persistent {
        db: sqlite::Connection,
        host_status: HostStatus,
    },
    InMemory {
        host_status: HostStatus,
    },
}

impl DataStore {
    pub(crate) fn new() -> Self {
        Self::InMemory {
            host_status: HostStatus::default(),
        }
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

        Ok(Self::Persistent { db, host_status })
    }

    pub(crate) fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent { .. })
    }

    pub(crate) fn persist(&mut self, path: &Path) -> Result<(), Error> {
        if let Self::InMemory {
            ref mut host_status,
        } = self
        {
            if path.exists() {
                bail!("Datastore already exists");
            }
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
            Self::write_host_status(&db, host_status)?;
            *self = Self::Persistent {
                db,
                host_status: mem::take(host_status),
            };
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
        match &self {
            Self::Persistent { host_status, .. } => host_status,
            Self::InMemory { host_status } => host_status,
        }
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

        match self {
            Self::Persistent { db, host_status } => {
                *host_status = updated;

                // Always attempt to save the updated host status, even if the previous call failed,
                // but only report errors from saving the host status if it succeeded.
                let ret2 = Self::write_host_status(db, host_status);
                if ret.is_ok() {
                    ret2?;
                }
            }
            Self::InMemory { host_status } => {
                *host_status = updated;
            }
        }

        ret
    }

    pub(crate) fn close(&mut self) {
        if let Self::Persistent { db: _, host_status } = self {
            *self = Self::InMemory {
                host_status: mem::take(host_status),
            };
        }
    }
}
