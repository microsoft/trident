use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Error};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct ProcessFiles {
    pub command: String,
    pub paths: Vec<PathBuf>,
}

pub fn run(directory_path: impl AsRef<Path>) -> Result<Vec<ProcessFiles>, Error> {
    let result = Command::new("lsof")
        .arg("-V") // report what could not be found
        .arg("-x") // controls handling of cross-over processing for symlinks and mounts
        .arg("f") // follow volume mounts (but not symlinks)
        .arg("+D") // and do it for the entire subtree under `directory_path`
        .arg(directory_path.as_ref()) // search recursively
        .arg("-F") // controls output format
        .arg("cn") // fetch command and name
        .output()
        .context("Failed to list opened files")?;
    // ignoring exit code, as lsof returns 1 if no open files are found for any
    // file in the subtree that is searched
    parse_lsof_output(&String::from_utf8_lossy(&result.stdout))
}

fn parse_lsof_output(output: &str) -> Result<Vec<ProcessFiles>, Error> {
    println!("parsing");
    let mut processes = Vec::new();
    let mut process: Option<ProcessFiles> = None;
    for line in output.lines() {
        if line.starts_with('c') {
            if let Some(process) = process {
                processes.push(process);
            }
            process = Some(ProcessFiles {
                command: line.strip_prefix('c').unwrap().into(),
                paths: Vec::new(),
            });
        } else if line.starts_with('n') {
            process
                .as_mut()
                .context("missing process name")?
                .paths
                .push(PathBuf::from(line.strip_prefix('n').unwrap()));
        }
    }
    if let Some(process) = process {
        processes.push(process);
    }
    Ok(processes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lsof_output() {
        let output = indoc::indoc!(
            r#"
            p228
            csystemd-journal
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal
            p414
            cjournalctl
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system@312132282b034b13bf33633d64e625ea-000000000000214d-00060d1de2c9294c.journal
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system@312132282b034b13bf33633d64e625ea-000000000000214d-00060d1de2c9294c.journal
            p9156
            ctrident
            n/var/lib/trident/tmp-datastore.sqlite
            "#
        );
        let expected_process_files_list = vec![
            ProcessFiles {
                command: "systemd-journal".into(),
                paths: vec![
                    PathBuf::from(
                        "/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal",
                    ),
                    PathBuf::from(
                        "/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal",
                    ),
                ],
            },
            ProcessFiles {
                command: "journalctl".into(),
                paths: vec![PathBuf::from(
                    "/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal",
                ),
                PathBuf::from("/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system@312132282b034b13bf33633d64e625ea-000000000000214d-00060d1de2c9294c.journal"),
                PathBuf::from("/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal"),
                PathBuf::from("/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system@312132282b034b13bf33633d64e625ea-000000000000214d-00060d1de2c9294c.journal"),
                ],
            },
            ProcessFiles {
                command: "trident".into(),
                paths: vec![
                    PathBuf::from(
                        "/var/lib/trident/tmp-datastore.sqlite",
                    ),
                ],
            },
        ];
        let process_files_list = parse_lsof_output(output).unwrap();
        assert_eq!(process_files_list, expected_process_files_list);

        assert_eq!(parse_lsof_output("bad output").unwrap(), Vec::new());

        // malformed output, missing process name
        let output = indoc::indoc!(
            r#"
            p228
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal
            csystemd-journal
            n/var/log/journal/a3355ae88df94601a7029fe157ccbee1/system.journal
            "#
        );
        assert_eq!(
            parse_lsof_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "missing process name"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use std::{
        fs::{self, File},
        path::Path,
    };

    use pytest_gen::functional_test;

    use crate::files::create_dirs;

    #[functional_test(feature = "helpers")]
    fn test_run_detects_open_files() {
        // create a temporary file and keep it open
        let dir_path = Path::new("/tmp/test-lsof");
        create_dirs(dir_path).unwrap();
        let file_path = dir_path.join("test-file");
        let file = File::create(&file_path).unwrap();

        // run lsof and check that the file is open
        let process_files_list = super::run(dir_path).unwrap();
        assert_eq!(process_files_list.len(), 1);
        assert_eq!(process_files_list[0].command, "osutils");
        assert_eq!(process_files_list[0].paths.len(), 1);
        assert_eq!(process_files_list[0].paths[0], file_path);

        // close the file and run lsof again
        drop(file);
        let process_files_list = super::run(dir_path).unwrap();
        assert_eq!(process_files_list.len(), 0);

        // remove the directory and run lsof again
        fs::remove_dir_all(dir_path).unwrap();
        let process_files_list = super::run(dir_path).unwrap();
        assert_eq!(process_files_list.len(), 0);
    }
}
