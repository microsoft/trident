use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::trace;
use regex::Regex;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Menuentry-aware grub.cfg parsing
// ---------------------------------------------------------------------------

/// Return the first whitespace-delimited word from a line, or None if the
/// line is empty / whitespace-only.
fn first_word(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

/// Extract the quoted title from the text after the `menuentry` keyword.
/// Handles both single and double quotes. Returns the content between the
/// first pair of matching quotes, or None if no quoted string is found.
fn extract_quoted_title(after_menuentry: &str) -> Option<&str> {
    let s = after_menuentry.trim();
    let quote = s.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let inner = &s[1..];
    let end = inner.find(quote)?;
    Some(&inner[..end])
}

/// Find `linux` directive lines from non-recovery menuentries in grub.cfg
/// content.
///
/// This is a **read-only** parser that walks the raw grub.cfg text
/// line-by-line looking for `menuentry` keywords, checks the quoted title
/// for the substring `"recovery"` (case-sensitive, matching the Go
/// implementation), and collects the arguments portion of each `linux`
/// line inside non-recovery blocks.
///
/// Returns all matches; callers decide whether to require exactly one.
///
/// # Distinction from [`GrubConfig`]
///
/// [`GrubConfig`] is a **read-write** abstraction that loads a grub.cfg
/// from disk and uses a regex to locate the single `linux` command line
/// (without menuentry-awareness). It is designed for in-place value
/// updates.
///
/// This function is a **read-only** content parser that understands
/// menuentry blocks and filters recovery entries. It operates on a raw
/// `&str` and does not modify anything.
///
/// # Known limitation (matches Go)
///
/// This parser does not track `submenu { ... }` block nesting or `{`/`}`
/// brace depth. On systems with multiple kernels, `grub2-mkconfig`
/// produces a top-level menuentry plus a `submenu 'Advanced options ...'`
/// block. Both Go's `FindNonRecoveryLinuxLine` and this function will find
/// >1 linux line, which may cause callers that expect exactly one to
/// error. This is acceptable because AZL images built by trident have
/// exactly one kernel installed.
pub fn find_non_recovery_linux_lines(content: &str) -> Result<Vec<String>, Error> {
    let mut in_menuentry = false;
    let mut linux_lines = Vec::new();

    for line in content.lines() {
        let keyword = match first_word(line) {
            Some(w) => w,
            None => continue,
        };

        if keyword == "menuentry" {
            in_menuentry = true;
            let after_keyword = line[line.find("menuentry").unwrap() + "menuentry".len()..].trim();
            if let Some(title) = extract_quoted_title(after_keyword) {
                if title.contains("recovery") {
                    in_menuentry = false;
                }
            }
        } else if in_menuentry && keyword == "linux" {
            let after_linux = line[line.find("linux").unwrap() + "linux".len()..].trim();
            if !after_linux.is_empty() {
                linux_lines.push(after_linux.to_string());
            }
        }
    }

    if linux_lines.is_empty() {
        bail!("no linux line found in non-recovery menuentry");
    }

    Ok(linux_lines)
}

// ---------------------------------------------------------------------------
// GrubConfig — read/write operations on a grub.cfg file
// ---------------------------------------------------------------------------

/// Represents the GRUB configuration file. Support simple validation and
/// retrieving and updating values. Temporary solution until we switch to more
/// structured configuration.
pub struct GrubConfig {
    path: PathBuf,
    contents: String,
    linux_command_line: Option<Vec<(String, String)>>,
}

// Match a full line, capture group 1 is the white space prefix ending with
// `linux `, capture group 2 is the suffix including all the arguments.
const LINUX_COMMAND_LINE_PATTERN: &str = r"(?m)^(\s*linux\s)(.+)$";

impl GrubConfig {
    /// Load grub.cfg from a disk.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        if !path.as_ref().exists() {
            bail!(
                "GRUB config does not exist at path: '{}'",
                path.as_ref().display()
            );
        }

        Ok(Self {
            path: path.as_ref().to_owned(),
            contents: fs::read_to_string(path.as_ref())
                .context(format!("Failed to read file '{}'", path.as_ref().display()))?,
            linux_command_line: None,
        })
    }

    /// Check if exactly one linux command line is present in the GRUB config.
    pub fn check_linux_command_line_count(&self) -> Result<(), Error> {
        let re = Regex::new(LINUX_COMMAND_LINE_PATTERN)?;
        let count = re.find_iter(&self.contents).count();
        if count == 0 {
            bail!("No linux command line found in '{}'", &self.path.display());
        } else if count > 1 {
            bail!(
                "Multiple linux command lines found in '{}'",
                &self.path.display()
            )
        }

        Ok(())
    }

    /// Find the linux command line in the GRUB config.
    fn find_linux_command_line(&self) -> Result<&str, Error> {
        let re = Regex::new(LINUX_COMMAND_LINE_PATTERN)?;
        let linux_command_line = re
            .captures(&self.contents)
            .context(format!(
                "Failed to find linux command line in '{}'",
                &self.path.display()
            ))?
            .get(2) // The list of arguments
            .context("No capture on linux command line")?
            .as_str();
        trace!("Found Linux command line: {}", linux_command_line);

        Ok(linux_command_line)
    }

    fn parse_linux_command_line(&self) -> Result<Vec<(String, String)>, Error> {
        self.find_linux_command_line()?
            .split_whitespace()
            .map(|arg| {
                let mut parts = arg.splitn(2, '=');
                Ok((
                    parts
                        .next()
                        .context("Failed to parse linux command line")?
                        .to_owned(),
                    parts.next().unwrap_or("").to_owned(),
                ))
            })
            .collect()
    }

    /// Checks if a specific argument is present in the linux command line
    pub fn contains_linux_command_line_argument(&mut self, key: &str) -> Result<bool, Error> {
        if self.linux_command_line.is_none() {
            self.linux_command_line = Some(self.parse_linux_command_line()?);
        }
        Ok(self
            .linux_command_line
            .as_ref()
            .unwrap()
            .iter()
            .any(|(k, _)| k == key))
    }

    /// Read a value of an argument from the linux command line in the GRUB
    /// config. If multiple matching keys are present, returns the value of the
    /// last key.
    pub fn read_linux_command_line_argument(&mut self, key: &str) -> Result<String, Error> {
        let linux_command_line = match &self.linux_command_line {
            Some(linux_command_line) => linux_command_line,
            None => {
                self.linux_command_line = Some(self.parse_linux_command_line()?);
                self.linux_command_line.as_ref().unwrap()
            }
        };
        linux_command_line
            .iter()
            .rev()
            .find_map(|(k, v)| if k == key { Some(v.clone()) } else { None })
            .context(format!(
                "Failed to find '{}' on linux command line in '{}'",
                key,
                self.path.display()
            ))
    }

    /// Serializes the linux command line from internal vector of pairs to a string.
    fn serialize_linux_command_line(&self) -> Result<String, Error> {
        Ok(self
            .linux_command_line
            .as_ref()
            .context("Linux command line not parsed")?
            .iter()
            .map(|(k, v)| match v.as_str() {
                "" => k.clone(),
                _ => format!("{k}={v}"),
            })
            .collect::<Vec<String>>()
            .join(" "))
    }

    /// Update a value of an argument in the linux command line in the GRUB, in
    /// the internal vector of pairs. If multiple matching keys are present,
    /// updates the value of the last key.
    fn update_linux_command_line_parsed(&mut self, key: &str, value: &str) -> Result<(), Error> {
        if self.linux_command_line.is_none() {
            bail!("Linux command line not parsed")
        }

        match self
            .linux_command_line
            .as_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|(k, _)| k == key)
        {
            Some((_, v)) => *v = value.to_owned(),
            None => {
                bail!(
                    "Unable to find {key} on linux command line in '{}'",
                    &self.path.display()
                )
            }
        }

        Ok(())
    }

    /// Update a value of an argument in the linux command line in the GRUB
    /// config. If multiple matching keys are present, updates the value of the
    /// last key.
    pub fn update_linux_command_line_argument(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<(), Error> {
        if self.linux_command_line.is_none() {
            self.linux_command_line = Some(self.parse_linux_command_line()?);
        }

        self.update_linux_command_line_parsed(key, value)?;
        self.update_linux_command_line()?;

        Ok(())
    }

    fn update_linux_command_line(&mut self) -> Result<(), Error> {
        let re = Regex::new(LINUX_COMMAND_LINE_PATTERN)?;
        let captures = re.captures(&self.contents).context(format!(
            "Failed to find linux command line in '{}'",
            &self.path.display()
        ))?;
        if captures.len() != 3 {
            bail!(
                "Failed to find linux command line in '{}', unexpected format",
                &self.path.display()
            )
        }
        // Capture group 2 gets the suffix behind `linux `
        let suffix_match = captures.get(2).context(format!(
            "Failed to find linux command line in '{}', missing arguments",
            &self.path.display()
        ))?;
        self.contents.replace_range(
            suffix_match.range(),
            self.serialize_linux_command_line()?.as_str(),
        );

        Ok(())
    }

    /// Insert a value of an argument into the linux command line in the GRUB
    /// config, at the end of the command line.
    pub fn append_linux_command_line_argument(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<(), Error> {
        if self.linux_command_line.is_none() {
            self.linux_command_line = Some(self.parse_linux_command_line()?);
        }

        self.linux_command_line
            .as_mut()
            .unwrap()
            .push((key.to_owned(), value.to_owned()));

        self.update_linux_command_line()?;

        Ok(())
    }

    /// Update the search command in the GRUB config.
    pub fn update_search(&mut self, uuid: &Uuid) -> Result<(), Error> {
        let re = Regex::new(r"(?m)^(\s*)search -n -u [\w-]+ -s$").unwrap();
        let re2 = Regex::new(r"(?m)^(\s*)search --no-floppy --fs-uuid --set=root [\w-]+$").unwrap();

        if re.is_match(&self.contents) {
            self.contents = re
                .replace(&self.contents, &format!("${{1}}search -n -u {uuid} -s"))
                .to_string();
        } else if re2.is_match(&self.contents) {
            self.contents = re2
                .replace(
                    &self.contents,
                    &format!("${{1}}search --no-floppy --fs-uuid --set=root {uuid}"),
                )
                .to_string();
        } else {
            bail!(
                "Unable to find search command in '{}'",
                &self.path.display()
            )
        }

        Ok(())
    }

    /// Update the set rootdevice command in the GRUB config.
    pub fn update_rootdevice(&mut self, root_device_path: impl AsRef<Path>) -> Result<(), Error> {
        let re = Regex::new(r"(?m)^(\s*)set rootdevice=[\w/=-]+$").unwrap();

        if !re.is_match(&self.contents) {
            bail!(
                "Unable to find set rootdevice command in '{}'",
                &self.path.display()
            )
        }

        let file_content = re
            .replace(
                &self.contents,
                &format!(
                    "${{1}}set rootdevice={}",
                    root_device_path
                        .as_ref()
                        .to_str()
                        .context("Failed to convert root device path to string")?
                        .trim()
                ),
            )
            .to_string();

        self.contents = file_content;

        Ok(())
    }

    /// Write the GRUB config back to disk
    pub fn write(&self) -> Result<(), Error> {
        fs::write(&self.path, &self.contents)
            .context(format!("Failed to write file '{}'", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn upstream_grubcfg() -> &'static str {
        indoc::indoc! { r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u c380c8e5-88ec-4c3e-85bb-aa1e4d667dfc -s

            load_env -f $bootprefix/mariner.cfg
            if [ -f $bootprefix/mariner-mshv.cfg ]; then
                    load_env -f $bootprefix/mariner-mshv.cfg
            fi

            if [ -f  $bootprefix/systemd.cfg ]; then
                    load_env -f $bootprefix/systemd.cfg
            else
                    set systemd_cmdline=net.ifnames=0
            fi
            if [ -f $bootprefix/grub2/grubenv ]; then
                    load_env -f $bootprefix/grub2/grubenv
            fi

            set rootdevice=PARTUUID=fc7675ee-37ce-471f-9a6c-7e840189b70c

            menuentry "CBL-Mariner" {
                    linux $bootprefix/$mariner_linux     security=selinux selinux=1  rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline  console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076
                    if [ -f $bootprefix/$mariner_initrd ]; then
                            initrd $bootprefix/$mariner_initrd
                    fi
            }
        "# }
    }

    #[test]
    fn test_update_linux_command_line_parsed() {
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: upstream_grubcfg().into(),
            linux_command_line: None,
        };

        assert_eq!(
            grub_config
                .update_linux_command_line_parsed("foo", "bar")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Linux command line not parsed"
        );

        grub_config.linux_command_line = Some(grub_config.parse_linux_command_line().unwrap());

        grub_config
            .update_linux_command_line_parsed("roothash", "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c")
            .unwrap();

        assert_eq!(
            grub_config.linux_command_line,
            Some(vec![
                ("$bootprefix/$mariner_linux".into(), "".into()),
                ("security".into(), "selinux".into()),
                ("selinux".into(), "1".into()),
                ("rd.auto".into(), "1".into()),
                ("root".into(), "$rootdevice".into()),
                ("$mariner_cmdline".into(), "".into()),
                ("lockdown".into(), "integrity".into()),
                ("sysctl.kernel.unprivileged_bpf_disabled".into(), "1".into()),
                ("$systemd_cmdline".into(), "".into()),
                ("console".into(), "tty0".into()),
                ("console".into(), "ttyS0".into()),
                ("$kernelopts".into(), "".into()),
                ("debug".into(), "".into()),
                (
                    "roothash".into(),
                    "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c".into()
                )
            ])
        );

        grub_config.linux_command_line = Some(vec![
            (
                "roothash".to_owned(),
                "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c".to_owned(),
            ),
            (
                "roothash".to_owned(),
                "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c".to_owned(),
            ),
            ("foo".to_owned(), "bar".to_owned()),
        ]);

        grub_config
            .update_linux_command_line_parsed("roothash", "9e6a9d2c-b7fe-4359-ac45-18b505e29d8d")
            .unwrap();

        assert_eq!(
            grub_config.linux_command_line,
            Some(vec![
                (
                    "roothash".to_owned(),
                    "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c".to_owned(),
                ),
                (
                    "roothash".to_owned(),
                    "9e6a9d2c-b7fe-4359-ac45-18b505e29d8d".to_owned(),
                ),
                ("foo".to_owned(), "bar".to_owned()),
            ])
        );

        // no update
        grub_config
            .update_linux_command_line_parsed("roothash", "9e6a9d2c-b7fe-4359-ac45-18b505e29d8d")
            .unwrap();

        assert_eq!(
            grub_config.linux_command_line,
            Some(vec![
                (
                    "roothash".to_owned(),
                    "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c".to_owned(),
                ),
                (
                    "roothash".to_owned(),
                    "9e6a9d2c-b7fe-4359-ac45-18b505e29d8d".to_owned(),
                ),
                ("foo".to_owned(), "bar".to_owned()),
            ])
        );

        // missing key
        assert_eq!(
            grub_config
                .update_linux_command_line_parsed("timeout", "10")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Unable to find timeout on linux command line in ''"
        );
    }

    #[test]
    fn test_serialize_linux_command_line() {
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: upstream_grubcfg().into(),
            linux_command_line: None,
        };

        assert_eq!(
            grub_config
                .serialize_linux_command_line()
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Linux command line not parsed"
        );

        grub_config.linux_command_line = Some(grub_config.parse_linux_command_line().unwrap());

        assert_eq!(
            grub_config.serialize_linux_command_line().unwrap(),
            "$bootprefix/$mariner_linux security=selinux selinux=1 rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076"
        );
    }

    #[test]
    fn test_read_update_write() {
        // Define original GRUB config contents on target machine
        let original_content_grub = upstream_grubcfg();
        // Create a temporary file with the original GRUB config contents
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_file_path = temp_dir.path().join("grub.cfg");
        fs::write(&temp_file_path, original_content_grub).unwrap();

        assert_eq!(
            GrubConfig::read("/does-not-exist")
                .err()
                .unwrap()
                .root_cause()
                .to_string(),
            "GRUB config does not exist at path: '/does-not-exist'"
        );

        let mut grub_config = GrubConfig::read(&temp_file_path).unwrap();
        assert_eq!(grub_config.contents, original_content_grub);

        // Define the expected GRUB config contents after the update
        let expected_content_grub = upstream_grubcfg();

        grub_config.contents = expected_content_grub.to_string();
        grub_config.write().unwrap();

        // Read the updated GRUB config
        let updated_content_grub = fs::read_to_string(&temp_file_path).unwrap();
        // Compare the updated GRUB config with the expected one
        assert_eq!(updated_content_grub, expected_content_grub);
    }

    #[test]
    fn test_check_linux() {
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        grub_config.check_linux_command_line_count().unwrap();

        // no linux
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            "#
        .to_owned();

        assert_eq!(
            grub_config
                .check_linux_command_line_count()
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No linux command line found in ''"
        );

        // too many penguins
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            linux roothash=9e6a9d2c-b7fe-4359-ac45-18b505e29d8b
                linux roothash=9e6a9d2c-b7fe-4359-ac45-18b505e29d8e
            "#
        .to_owned();

        assert_eq!(
            grub_config
                .check_linux_command_line_count()
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Multiple linux command lines found in ''"
        );
    }

    #[test]
    fn test_find_linux_command_line() {
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        assert_eq!(
            grub_config.find_linux_command_line().unwrap(),
            "$bootprefix/$mariner_linux     security=selinux selinux=1  rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline  console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076"
        );

        // no linux
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            "#
        .to_owned();

        assert_eq!(
            grub_config
                .find_linux_command_line()
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find linux command line in ''"
        );
    }

    #[test]
    fn test_contains_linux_command_line_argument() {
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        assert!(grub_config
            .contains_linux_command_line_argument("roothash")
            .unwrap());

        // missing value
        assert!(!grub_config
            .contains_linux_command_line_argument("timeout")
            .unwrap());

        // no linux
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            "#
        .to_owned();
        grub_config.linux_command_line = None;

        assert_eq!(
            grub_config
                .contains_linux_command_line_argument(r"roothash")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find linux command line in ''"
        );
    }

    #[test]
    fn test_read_linux_command_line_argument() {
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        assert_eq!(
            grub_config
                .read_linux_command_line_argument("roothash")
                .unwrap(),
            "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076"
        );

        // missing value
        assert_eq!(
            grub_config
                .read_linux_command_line_argument("timeout")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find 'timeout' on linux command line in ''"
        );

        // no linux
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            "#
        .to_owned();
        grub_config.linux_command_line = None;

        assert_eq!(
            grub_config
                .read_linux_command_line_argument(r"roothash")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find linux command line in ''"
        );
    }

    #[test]
    fn test_update_linux_command_line_argument() {
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        grub_config
            .update_linux_command_line_argument("roothash", "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c")
            .unwrap();

        assert_eq!(
            grub_config.contents,
            indoc::indoc! { r#"
                set timeout=0
                set bootprefix=/boot
                search -n -u c380c8e5-88ec-4c3e-85bb-aa1e4d667dfc -s

                load_env -f $bootprefix/mariner.cfg
                if [ -f $bootprefix/mariner-mshv.cfg ]; then
                        load_env -f $bootprefix/mariner-mshv.cfg
                fi

                if [ -f  $bootprefix/systemd.cfg ]; then
                        load_env -f $bootprefix/systemd.cfg
                else
                        set systemd_cmdline=net.ifnames=0
                fi
                if [ -f $bootprefix/grub2/grubenv ]; then
                        load_env -f $bootprefix/grub2/grubenv
                fi

                set rootdevice=PARTUUID=fc7675ee-37ce-471f-9a6c-7e840189b70c

                menuentry "CBL-Mariner" {
                        linux $bootprefix/$mariner_linux security=selinux selinux=1 rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts debug roothash=9e6a9d2c-b7fe-4359-ac45-18b505e29d8c
                        if [ -f $bootprefix/$mariner_initrd ]; then
                                initrd $bootprefix/$mariner_initrd
                        fi
                }
            "# }
        );

        // no update
        grub_config
            .update_linux_command_line_argument("roothash", "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c")
            .unwrap();

        // outside of linux
        assert_eq!(
            grub_config
                .update_linux_command_line_argument("timeout", "10")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Unable to find timeout on linux command line in ''"
        );

        // no linux
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            "#
        .to_owned();
        grub_config.linux_command_line = None;

        assert_eq!(
            grub_config
                .update_linux_command_line_argument(
                    "roothash",
                    "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c"
                )
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find linux command line in ''"
        );
    }

    #[test]
    fn test_append_linux_command_line_argument() {
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        grub_config
            .append_linux_command_line_argument("roothash", "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c")
            .unwrap();

        assert_eq!(
            grub_config.contents,
            indoc::indoc! { r#"
                set timeout=0
                set bootprefix=/boot
                search -n -u c380c8e5-88ec-4c3e-85bb-aa1e4d667dfc -s

                load_env -f $bootprefix/mariner.cfg
                if [ -f $bootprefix/mariner-mshv.cfg ]; then
                        load_env -f $bootprefix/mariner-mshv.cfg
                fi

                if [ -f  $bootprefix/systemd.cfg ]; then
                        load_env -f $bootprefix/systemd.cfg
                else
                        set systemd_cmdline=net.ifnames=0
                fi
                if [ -f $bootprefix/grub2/grubenv ]; then
                        load_env -f $bootprefix/grub2/grubenv
                fi

                set rootdevice=PARTUUID=fc7675ee-37ce-471f-9a6c-7e840189b70c

                menuentry "CBL-Mariner" {
                        linux $bootprefix/$mariner_linux security=selinux selinux=1 rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076 roothash=9e6a9d2c-b7fe-4359-ac45-18b505e29d8c
                        if [ -f $bootprefix/$mariner_initrd ]; then
                                initrd $bootprefix/$mariner_initrd
                        fi
                }
            "# }
        );

        // new argument
        grub_config
            .append_linux_command_line_argument("foobar", "barfoo")
            .unwrap();

        assert_eq!(
            grub_config.contents,
            indoc::indoc! { r#"
                set timeout=0
                set bootprefix=/boot
                search -n -u c380c8e5-88ec-4c3e-85bb-aa1e4d667dfc -s

                load_env -f $bootprefix/mariner.cfg
                if [ -f $bootprefix/mariner-mshv.cfg ]; then
                        load_env -f $bootprefix/mariner-mshv.cfg
                fi

                if [ -f  $bootprefix/systemd.cfg ]; then
                        load_env -f $bootprefix/systemd.cfg
                else
                        set systemd_cmdline=net.ifnames=0
                fi
                if [ -f $bootprefix/grub2/grubenv ]; then
                        load_env -f $bootprefix/grub2/grubenv
                fi

                set rootdevice=PARTUUID=fc7675ee-37ce-471f-9a6c-7e840189b70c

                menuentry "CBL-Mariner" {
                        linux $bootprefix/$mariner_linux security=selinux selinux=1 rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076 roothash=9e6a9d2c-b7fe-4359-ac45-18b505e29d8c foobar=barfoo
                        if [ -f $bootprefix/$mariner_initrd ]; then
                                initrd $bootprefix/$mariner_initrd
                        fi
                }
            "# }
        );

        // outside of linux
        grub_config
            .append_linux_command_line_argument("timeout", "10")
            .unwrap();

        assert_eq!(
            grub_config.contents,
            indoc::indoc! { r#"
                set timeout=0
                set bootprefix=/boot
                search -n -u c380c8e5-88ec-4c3e-85bb-aa1e4d667dfc -s

                load_env -f $bootprefix/mariner.cfg
                if [ -f $bootprefix/mariner-mshv.cfg ]; then
                        load_env -f $bootprefix/mariner-mshv.cfg
                fi

                if [ -f  $bootprefix/systemd.cfg ]; then
                        load_env -f $bootprefix/systemd.cfg
                else
                        set systemd_cmdline=net.ifnames=0
                fi
                if [ -f $bootprefix/grub2/grubenv ]; then
                        load_env -f $bootprefix/grub2/grubenv
                fi

                set rootdevice=PARTUUID=fc7675ee-37ce-471f-9a6c-7e840189b70c

                menuentry "CBL-Mariner" {
                        linux $bootprefix/$mariner_linux security=selinux selinux=1 rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076 roothash=9e6a9d2c-b7fe-4359-ac45-18b505e29d8c foobar=barfoo timeout=10
                        if [ -f $bootprefix/$mariner_initrd ]; then
                                initrd $bootprefix/$mariner_initrd
                        fi
                }
            "# }
        );

        // no linux
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            "#
        .to_owned();
        grub_config.linux_command_line = None;

        assert_eq!(
            grub_config
                .append_linux_command_line_argument(
                    "roothash",
                    "9e6a9d2c-b7fe-4359-ac45-18b505e29d8c"
                )
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find linux command line in ''"
        );
    }

    #[test]
    fn test_update_search() {
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        grub_config
            .update_search(&Uuid::parse_str("9e6a9d2c-b7fe-4359-ac45-18b505e29d8c").unwrap())
            .unwrap();

        let expected_content_grub = indoc::indoc! { r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8c -s

            load_env -f $bootprefix/mariner.cfg
            if [ -f $bootprefix/mariner-mshv.cfg ]; then
                    load_env -f $bootprefix/mariner-mshv.cfg
            fi

            if [ -f  $bootprefix/systemd.cfg ]; then
                    load_env -f $bootprefix/systemd.cfg
            else
                    set systemd_cmdline=net.ifnames=0
            fi
            if [ -f $bootprefix/grub2/grubenv ]; then
                    load_env -f $bootprefix/grub2/grubenv
            fi

            set rootdevice=PARTUUID=fc7675ee-37ce-471f-9a6c-7e840189b70c

            menuentry "CBL-Mariner" {
                    linux $bootprefix/$mariner_linux     security=selinux selinux=1  rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline  console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076
                    if [ -f $bootprefix/$mariner_initrd ]; then
                            initrd $bootprefix/$mariner_initrd
                    fi
            }
        "# };

        assert_eq!(grub_config.contents, expected_content_grub);

        // no update
        grub_config
            .update_search(&Uuid::parse_str("9e6a9d2c-b7fe-4359-ac45-18b505e29d8c").unwrap())
            .unwrap();

        assert_eq!(grub_config.contents, expected_content_grub);

        // no search
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            "#
        .to_owned();

        assert_eq!(
            grub_config
                .update_search(&Uuid::parse_str("9e6a9d2c-b7fe-4359-ac45-18b505e29d8c").unwrap())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Unable to find search command in ''"
        );

        grub_config.contents = upstream_grubcfg().to_owned();
        grub_config
            .update_search(&Uuid::parse_str("c380c8e5-88ec-4c3e-85bb-aa1e4d667dff").unwrap())
            .unwrap();
    }

    #[test]
    fn test_update_rootdevice() {
        // Define original GRUB config contents on target machine
        let original_content_grub = upstream_grubcfg();
        let mut grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: original_content_grub.to_string(),
            linux_command_line: None,
        };

        grub_config
            .update_rootdevice(Path::new("/dev/sda1"))
            .unwrap();

        assert_eq!(
            grub_config.contents,
            indoc::indoc! { r#"
                set timeout=0
                set bootprefix=/boot
                search -n -u c380c8e5-88ec-4c3e-85bb-aa1e4d667dfc -s

                load_env -f $bootprefix/mariner.cfg
                if [ -f $bootprefix/mariner-mshv.cfg ]; then
                        load_env -f $bootprefix/mariner-mshv.cfg
                fi

                if [ -f  $bootprefix/systemd.cfg ]; then
                        load_env -f $bootprefix/systemd.cfg
                else
                        set systemd_cmdline=net.ifnames=0
                fi
                if [ -f $bootprefix/grub2/grubenv ]; then
                        load_env -f $bootprefix/grub2/grubenv
                fi

                set rootdevice=/dev/sda1

                menuentry "CBL-Mariner" {
                        linux $bootprefix/$mariner_linux     security=selinux selinux=1  rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline  console=tty0 console=ttyS0 $kernelopts debug roothash=4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076
                        if [ -f $bootprefix/$mariner_initrd ]; then
                                initrd $bootprefix/$mariner_initrd
                        fi
                }
            "#}
        );

        // no update
        grub_config
            .update_rootdevice(Path::new("/dev/sda1"))
            .unwrap();

        // no rootdevice
        grub_config.contents = r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
            linux roothash=9e6a9d2c-b7fe-4359-ac45-18b505e29d8b
            "#
        .to_owned();

        assert_eq!(
            grub_config
                .update_rootdevice(Path::new("/dev/sda1"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Unable to find set rootdevice command in ''"
        );
    }

    #[test]
    fn test_parse_linux_command_line() {
        let grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: upstream_grubcfg().into(),
            linux_command_line: None,
        };

        assert_eq!(
            grub_config.parse_linux_command_line().unwrap(),
            vec![
                ("$bootprefix/$mariner_linux".into(), "".into()),
                ("security".into(), "selinux".into()),
                ("selinux".into(), "1".into()),
                ("rd.auto".into(), "1".into()),
                ("root".into(), "$rootdevice".into()),
                ("$mariner_cmdline".into(), "".into()),
                ("lockdown".into(), "integrity".into()),
                ("sysctl.kernel.unprivileged_bpf_disabled".into(), "1".into()),
                ("$systemd_cmdline".into(), "".into()),
                ("console".into(), "tty0".into()),
                ("console".into(), "ttyS0".into()),
                ("$kernelopts".into(), "".into()),
                ("debug".into(), "".into()),
                (
                    "roothash".into(),
                    "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076".into()
                )
            ]
        );

        // no linux
        let grub_config = GrubConfig {
            path: PathBuf::new(),
            contents: r#"
                set timeout=0
                set bootprefix=/boot
                search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s
                "#
            .into(),
            linux_command_line: None,
        };

        assert_eq!(
            grub_config
                .parse_linux_command_line()
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find linux command line in ''"
        );
    }

    // ============= find_non_recovery_linux_lines =============

    #[test]
    fn test_non_recovery_with_recovery_entry() {
        let grub_cfg = indoc::indoc! {r#"
            set timeout=5
            menuentry 'Azure Linux' --class azurelinux {
                linux /boot/vmlinuz root=/dev/sda2 selinux=1 enforcing=1 rd.overlayfs=/a,/b,/c,/dev/sda3
                initrd /boot/initrd.img
            }
            menuentry 'Azure Linux (recovery)' --class azurelinux {
                linux /boot/vmlinuz root=/dev/sda2 single
                initrd /boot/initrd.img
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        let result = &lines[0];
        assert!(result.contains("root=/dev/sda2"));
        assert!(result.contains("selinux=1"));
        assert!(result.contains("rd.overlayfs="));
        assert!(!result.contains("single"));
    }

    #[test]
    fn test_single_non_recovery_entry() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda1"));
    }

    #[test]
    fn test_no_linux_line_errors() {
        let grub_cfg = "set timeout=5\n";
        assert!(find_non_recovery_linux_lines(grub_cfg).is_err());
    }

    #[test]
    fn test_only_recovery_entries_errors() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux (recovery)' {
                linux /boot/vmlinuz root=/dev/sda1 single
            }
        "#};
        assert!(find_non_recovery_linux_lines(grub_cfg).is_err());
    }

    #[test]
    fn test_recovery_detection_is_case_sensitive() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux Recovery Mode' {
                linux /boot/vmlinuz root=/dev/sda1 single
            }
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda2
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(
            lines.len(),
            2,
            "uppercase 'Recovery' should not be filtered"
        );
    }

    #[test]
    fn test_multiple_non_recovery_entries() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux A' {
                linux /boot/vmlinuz root=/dev/sda1
            }
            menuentry 'Linux B' {
                linux /boot/vmlinuz root=/dev/sda2
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_linux_line_captures_full_args() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda2 selinux=1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert!(lines[0].starts_with("/boot/vmlinuz"));
        assert!(lines[0].contains("selinux=1"));
    }

    #[test]
    fn test_tab_indented_grub_cfg() {
        let grub_cfg = "menuentry 'Linux' {\n\tlinux /boot/vmlinuz root=/dev/sda2 selinux=1\n\tinitrd /boot/initrd.img\n}\n";

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda2"));
    }

    #[test]
    fn test_double_quoted_menuentry_title() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry "Azure Linux" {
                linux /boot/vmlinuz root=/dev/sda1
            }
            menuentry "Azure Linux (recovery)" {
                linux /boot/vmlinuz root=/dev/sda1 single
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].contains("single"));
    }

    #[test]
    fn test_recovery_in_class_not_in_title() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Azure Linux' --class recovery-icon {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(
            lines.len(),
            1,
            "recovery in class name should not filter the entry"
        );
    }

    #[test]
    fn test_real_world_azl2_grub_cfg() {
        let grub_cfg = indoc::indoc! {r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 33beac00-b378-4b0c-b0cb-d5dcebf2cf57 -s

            load_env -f $bootprefix/mariner.cfg

            set rootdevice=PARTUUID=c17c558b-068b-459c-92cb-f218d14b44a1

            menuentry "CBL-Mariner" {
            	linux $bootprefix/$mariner_linux       rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity selinux=0 $systemd_cmdline   $kernelopts
            	if [ -f $bootprefix/$mariner_initrd ]; then
            		initrd $bootprefix/$mariner_initrd
            	fi
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("selinux=0"));
        assert!(lines[0].contains("root=$rootdevice"));
    }

    #[test]
    fn test_menuentry_without_linux_line() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Empty Entry' {
                set gfxpayload=keep
            }
            menuentry 'Real Entry' {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda1"));
    }

    #[test]
    fn test_linux_outside_menuentry_ignored() {
        let grub_cfg = indoc::indoc! {r#"
            linux /boot/stray-vmlinuz root=/dev/stray
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda1"));
    }
}
