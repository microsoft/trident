use clap::Parser;

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::HandleCommand;

#[derive(Parser, Debug)]
pub struct User {
    #[clap(skip)]
    pub line: KSLine,

    #[arg(long, default_value = "/home/")]
    homedir: String,

    #[arg(
        long,
        group = "password",
        requires = "password",
        default_value_if("plaintext", "false", "true")
    )]
    iscrypted: bool,

    #[arg(long, required = true)]
    name: String,

    #[arg(long)]
    password: Option<String>,

    #[arg(long)]
    shell: Option<String>,

    #[arg(long)]
    uid: Option<u32>,

    #[arg(long, group = "password", conflicts_with = "password")]
    lock: bool,

    #[arg(long, group = "password", requires = "password")]
    plaintext: bool,

    #[arg(long)]
    gecos: Option<String>,

    #[arg(long)]
    gid: Option<u32>,

    #[arg(long, value_delimiter = ',')]
    groups: Option<Vec<String>>,
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
