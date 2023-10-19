use clap::Parser;

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::HandleCommand;

/// Create and configure a new user on the system.
#[derive(Parser, Debug)]
#[command(name = "user", help_expected = true)]
pub struct User {
    #[clap(skip)]
    pub line: KSLine,

    /// The user's home directory.
    #[arg(long, default_value = "/home/")]
    pub homedir: String,

    /// States the password provided is already encrypted.
    #[arg(
        long,
        group = "password",
        requires = "password",
        default_value_if("plaintext", "false", "true")
    )]
    pub iscrypted: bool,

    /// Locks the user account.
    #[arg(long, group = "password", conflicts_with = "password")]
    pub lock: bool,

    /// States the password provided is in plain text. (Default)
    #[arg(long, group = "password", requires = "password")]
    pub plaintext: bool,

    /// Name of the new user.
    #[arg(long, required = true)]
    pub name: String,

    /// The password to set for the user account.
    #[arg(long)]
    pub password: Option<String>,

    /// The user's login shell. If not provided, the system default will be used.
    #[arg(long)]
    pub shell: Option<String>,

    /// The user's UID. If not provided, the next available UID will be used.
    #[arg(long)]
    pub uid: Option<u32>,

    /// Provides the GECOS information for the user.
    #[arg(long)]
    pub gecos: Option<String>,

    /// The GID of the user’s primary group.
    #[arg(long)]
    pub gid: Option<u32>,

    /// In addition to the default group, a comma separated list of group names the
    /// user should belong to. Any groups that do not already exist will be created.
    /// If the group already exists with a different GID, an error will be raised.
    #[arg(long, value_delimiter = ',')]
    pub groups: Option<Vec<String>>,
}

impl HandleCommand for User {
    fn handle(mut self, line: KSLine, data: &mut ParsedData) -> Result<(), SetsailError> {
        if self.name == "root" {
            return Err(SetsailError::new_semantic(
                line,
                "user command cannot be used on root used, use rootpw instead".into(),
            ));
        }

        let mut result = Ok(());

        if let Some(old) = data.users.get(&self.name) {
            result = Err(SetsailError::new_sem_warn(
                line.clone(),
                format!("overriding previous rootpw command at {}", old.line),
            ));
        }

        self.line = line;
        data.users.insert(self.name.clone(), self);

        result
    }
}
