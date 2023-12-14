use trident_api::config::{HostConfiguration, Password, User};

use crate::{data::ParsedData, SetsailError};

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration, errors: &mut Vec<SetsailError>) {
    if let Some(ref root_data) = input.root {
        // Figure out the root password.

        let password = if root_data.lock {
            // If --lock was passed, lock the root password.
            Password::Locked
        } else if let Some(ref _password) = root_data.password {
            // If a password was passed, use it if passwords are allowed.
            #[cfg(feature = "dangerous-options")]
            if root_data.iscrypted {
                Password::DangerousHashed(_password.clone())
            } else {
                Password::DangerousPlainText(_password.clone())
            }

            // If a password was passed, report the error if passwords are
            // disabled.
            #[cfg(not(feature = "dangerous-options"))]
            {
                errors.push(SetsailError::new_disallowed_command(
                    root_data.line.clone(),
                    "Password authentication is not allowed".to_string(),
                ));

                return;
            }
        } else {
            // This should be unreachable (the parsing should fail), but we'll
            // handle it anyway.
            errors.push(SetsailError::new_translation(
                root_data.line.clone(),
                "Root password is required".to_string(),
            ));

            return;
        };

        hc.osconfig.users.push(User {
            name: "root".into(),
            password,
            ..Default::default()
        });
    }
}
