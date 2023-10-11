use trident_api::config::{HostConfiguration, Password, User};

use crate::{data::ParsedData, SetsailError};

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration, _errors: &mut [SetsailError]) {
    if let Some(ref root_data) = input.root {
        let mut usr = User::default();
        if root_data.lock {
            usr.password = Password::Locked;
        } else if let Some(ref password) = root_data.password {
            if root_data.iscrypted {
                usr.password = Password::DangerousHashed(password.clone());
            } else {
                usr.password = Password::DangerousPlainText(password.clone());
            }
        }

        hc.osconfig.users.insert("root".into(), usr);
    }
}
