package tests

import (
	"tridenttools/storm/servicing/utils/config"
	"tridenttools/storm/servicing/utils/ssh"
	"tridenttools/storm/servicing/utils/vmip"

	"github.com/sirupsen/logrus"
)

func FetchLogs(cfg config.ServicingConfig) error {
	vmIP, err := vmip.GetVmIP(cfg)
	if err != nil {
		return err
	}
	// Best effort: download journal log
	logrus.Tracef("Make journal log available for download")
	_, err = ssh.SshCommand(cfg.VMConfig, vmIP, "sudo journalctl --no-pager > /tmp/journal.log && sudo chmod 644 /tmp/journal.log")
	if err == nil {
		// Download file via scp if creating journal.log succeeded
		logrus.Tracef("Downloading journal log from VM '%s' to local machine", cfg.VMConfig.Name)
		ssh.ScpDownloadFile(cfg.VMConfig, vmIP, "/tmp/journal.log", cfg.TestConfig.OutputPath+"/journal.log")
	}
	// Download crashdumps (simplified)
	logrus.Tracef("Check for crash dumps on VM")
	crashDumpOutput, err := ssh.SshCommand(cfg.VMConfig, vmIP, "ls /var/crash/*")
	if err == nil {
		logrus.Debugf("Crash files found on host: %s", crashDumpOutput)
		logrus.Error("Crash files found on host")
		ssh.SshCommand(cfg.VMConfig, vmIP, "sudo mv /var/crash/* /tmp/crash && sudo chmod -R 644 /tmp/crash && sudo chmod +x /tmp/crash")
		ssh.ScpDownloadFile(cfg.VMConfig, vmIP, "/tmp/crash/*", cfg.TestConfig.OutputPath+"/")
	}
	return nil
}
