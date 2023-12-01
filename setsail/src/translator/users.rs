use trident_api::config::{HostConfiguration, Password, User};

use crate::{data::ParsedData, SetsailError};

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration, errors: &mut Vec<SetsailError>) {
    if let Some(ref root_data) = input.root {
        let mut usr = User::default();
        if root_data.lock {
            usr.password = Password::Locked;
        } else if let Some(ref _password) = root_data.password {
            #[cfg(feature = "dangerous-options")]
            if root_data.iscrypted {
                usr.password = Password::DangerousHashed(_password.clone());
            } else {
                usr.password = Password::DangerousPlainText(_password.clone());
            }

            #[cfg(not(feature = "dangerous-options"))]
            errors.push(SetsailError::new_disallowed_command(
                root_data.line.clone(),
                "Password authentication is not allowed".to_string(),
            ))
        }

        hc.osconfig.users.insert("root".into(), usr);
    }
}
