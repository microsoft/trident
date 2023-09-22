use clap::Parser;
use log::debug;

pub mod misc;

pub mod bootloader;
pub mod liveimg;
pub mod network;
pub mod partition;
pub mod rootpw;
pub mod services;
pub mod timezone;
pub mod user;

use super::{data::ParsedData, errors::ToResultSetsailError, types::KSLine, SetsailError};

pub struct CommandHandler<'a> {
    tokens: Vec<String>,
    line: KSLine,
    data: &'a mut ParsedData,
}

impl<'a> CommandHandler<'a> {
    pub fn new(tokens: Vec<String>, line: KSLine, data: &'a mut ParsedData) -> CommandHandler<'a> {
        Self { tokens, line, data }
    }

    fn dispatch<T: CommandProcessor>(self) -> Result<(), SetsailError> {
        T::process(self.tokens, self.line, self.data)
    }

    pub fn handle(mut self) -> Result<(), SetsailError> {
        match self.tokens[0].as_str() {
            // Commands we understand
            "bootloader" => self.dispatch::<bootloader::Bootloader>(),
            "liveimg" => self.dispatch::<liveimg::LiveImg>(),
            "network" => self.dispatch::<network::Network>(),
            "partition" | "part" => self.dispatch::<partition::Partition>(),
            "rootpw" => self.dispatch::<rootpw::Rootpw>(),
            "services" => self.dispatch::<services::Services>(),
            "timezone" => self.dispatch::<timezone::Timezone>(),
            "user" => self.dispatch::<user::User>(),

            // These are valid kickstart commands that we don't support
            // List from: https://pykickstart.readthedocs.io/en/latest/kickstart-docs.html
            "auth" | "authconfig" | "authselect" | "autopart" | "autostep" | "btrfs" | "cdrom"
            | "clearpart" | "graphical" | "text" | "cmdline" | "device" | "deviceprobe"
            | "dmraid" | "driverdisk" | "eula" | "fcoe" | "firewall" | "firstboot" | "group"
            | "reboot" | "poweroff" | "shutdown" | "halt" | "harddrive" | "hmc" | "ignoredisk"
            | "install" | "interactive" | "iscsi" | "iscsiname" | "keyboard" | "lang"
            | "langsupport" | "lilo" | "lilocheck" | "logging" | "logvol" | "mediacheck"
            | "method" | "module" | "monitor" | "mount" | "mouse" | "multipath" | "nfs"
            | "nvdimm" | "ostreecontainer" | "ostreesetup" | "raid" | "realm" | "repo"
            | "reqpart" | "rescue" | "selinux" | "skipx" | "snapshot" | "sshkey" | "sshpw"
            | "timesource" | "updates" | "upgrade" | "url" | "vnc" | "volgroup" | "xconfig"
            | "zerombr" | "zfcp" | "zipl" => Err(SetsailError::new_unsupported_command(
                self.line,
                self.tokens.swap_remove(0),
            )),

            // Everything else
            _ => Err(SetsailError::new_unknown_command(
                self.line,
                self.tokens.swap_remove(0),
            )),
        }
    }
}

/// Trait implemented by all command types
/// Each command type must implement this trait
/// They are expected to override the handle() method
/// to save their own data to the ParsedData object
trait HandleCommand: Sized + core::fmt::Debug {
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

/// Trait implemented by all command types
/// It contains all the share behavior around
trait CommandProcessor: Sized {
    fn process(
        tokens: Vec<String>,
        line: KSLine,
        data: &mut ParsedData,
    ) -> Result<(), SetsailError>;
}

impl<T> CommandProcessor for T
where
    T: Parser + HandleCommand,
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
