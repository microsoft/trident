use clap::Parser;

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::HandleCommand;

#[derive(Parser, Debug, Default)]
pub struct Rootpw {
    #[clap(skip)]
    pub line: KSLine,

    /// States the password provided is already encrypted.
    #[arg(long, group = "src", requires = "password")]
    pub iscrypted: bool,

    /// States the password provided is in plain text. (Default)
    #[arg(
        long,
        group = "src",
        requires = "password",
        default_value_if("plaintext", "false", "true")
    )]
    pub plaintext: bool,

    /// Locks the root account.
    #[arg(long, group = "src", conflicts_with = "password")]
    pub lock: bool,

    /// The password to set for the root account.
    #[arg(required_unless_present = "lock")]
    pub password: Option<String>,
}

impl HandleCommand for Rootpw {
    fn handle(mut self, line: KSLine, data: &mut ParsedData) -> Result<(), SetsailError> {
        let mut result = Ok(());
        if let Some(old) = &data.root {
            result = Err(SetsailError::new_sem_warn(
                line.clone(),
                format!("overriding previous rootpw command at {}", old.line),
            ));
        }

        self.line = line;
        data.root = Some(self);
        result
    }
}
