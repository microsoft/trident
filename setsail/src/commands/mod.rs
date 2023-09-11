use clap::Parser;
use log::debug;

mod bootloader;
mod liveimg;
mod partition;
mod rootpw;
mod services;
mod timezone;
mod user;

pub use bootloader::Bootloader;
pub use liveimg::LiveImg;
pub use partition::Partition;
pub use rootpw::Rootpw;
pub use services::Services;
pub use timezone::Timezone;
pub use user::User;

use super::{errors::ToResultSetsailError, parser::ParsedData, types::KSLine, SetsailError};

pub fn handle_command(
    line: KSLine,
    mut tokens: Vec<String>,
    data: &mut ParsedData,
) -> Result<(), SetsailError> {
    match tokens[0].as_str() {
        // Commands we understand
        //TODO: Implement command handling
        "bootloader" => Bootloader::process(tokens, line, data),
        "liveimg" => LiveImg::process(tokens, line, data),
        "partition" | "part" => Partition::process(tokens, line, data),
        "rootpw" => Rootpw::process(tokens, line, data),
        "services" => Services::process(tokens, line, data),
        "timezone" => Timezone::process(tokens, line, data),
        "user" => User::process(tokens, line, data),

        // These are valid kickstart commands that we don't support
        // List from: https://pykickstart.readthedocs.io/en/latest/kickstart-docs.html
        "auth" | "authconfig" | "authselect" | "autopart" | "autostep" | "btrfs" | "cdrom"
        | "clearpart" | "graphical" | "text" | "cmdline" | "device" | "deviceprobe" | "dmraid"
        | "driverdisk" | "eula" | "fcoe" | "firewall" | "firstboot" | "group" | "reboot"
        | "poweroff" | "shutdown" | "halt" | "harddrive" | "hmc" | "ignoredisk" | "install"
        | "interactive" | "iscsi" | "iscsiname" | "keyboard" | "lang" | "langsupport" | "lilo"
        | "lilocheck" | "logging" | "logvol" | "mediacheck" | "method" | "module" | "monitor"
        | "mount" | "mouse" | "multipath" | "network" | "nfs" | "nvdimm" | "ostreecontainer"
        | "ostreesetup" | "raid" | "realm" | "repo" | "reqpart" | "rescue" | "selinux"
        | "skipx" | "snapshot" | "sshkey" | "sshpw" | "timesource" | "updates" | "upgrade"
        | "url" | "vnc" | "volgroup" | "xconfig" | "zerombr" | "zfcp" | "zipl" => Err(
            SetsailError::new_unsupported_command(line, tokens.swap_remove(0)),
        ),

        // Everything else
        _ => Err(SetsailError::new_unknown_command(
            line,
            tokens.swap_remove(0),
        )),
    }
}

trait CommandHandler: Sized + core::fmt::Debug {
    fn handle(self, _: KSLine, _: &mut ParsedData) -> Result<(), SetsailError> {
        debug!(
            "Handling {} command: {:?}",
            std::any::type_name::<Self>()
                .split("::")
                .last()
                .unwrap_or("??"),
            self
        );
        Ok(())
    }
}

trait CommandProcessor: Sized {
    fn process(
        tokens: Vec<String>,
        line: KSLine,
        data: &mut ParsedData,
    ) -> Result<(), SetsailError>;
}

impl<T> CommandProcessor for T
where
    T: Parser + CommandHandler,
{
    fn process(
        tokens: Vec<String>,
        line: KSLine,
        data: &mut ParsedData,
    ) -> Result<(), SetsailError> {
        let cmd = T::try_parse_from(tokens).to_result_parser_error(&line)?;
        cmd.handle(line, data)
    }
}
