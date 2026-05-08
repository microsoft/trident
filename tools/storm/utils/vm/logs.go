package vm

import (
	stormssh "tridenttools/storm/utils/ssh"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

func FetchLogs(vmConfig stormvmconfig.AllVMConfig, outputPath string) error {
	vmIP, err := GetVmIP(vmConfig)
	if err != nil {
		return err
	}
	// Best effort: download journal log
	logrus.Tracef("Make journal log available for download")
	_, err = stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo journalctl --no-pager > /tmp/journal.log && sudo chmod 644 /tmp/journal.log")
	if err == nil {
		// Download file via scp if creating journal.log succeeded
		logrus.Tracef("Downloading journal log from VM '%s' to local machine", vmConfig.VMConfig.Name)
		stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, "/tmp/journal.log", outputPath+"/journal.log")
	}

	// Best effort: capture block device UUIDs for initramfs debugging
	logrus.Tracef("Capturing blkid output for initramfs diagnostics")
	if blkidOut, blkidErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo blkid > /tmp/blkid.log 2>&1 && sudo chmod 644 /tmp/blkid.log"); blkidErr == nil {
		logrus.Tracef("blkid output: %s", blkidOut)
		stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, "/tmp/blkid.log", outputPath+"/blkid.log")
	}

	// Best effort: capture initramfs contents to detect stale UUID references
	logrus.Tracef("Capturing lsinitrd output for initramfs diagnostics")
	if _, lsinitrdErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo lsinitrd 2>/dev/null > /tmp/lsinitrd.log 2>&1 && sudo chmod 644 /tmp/lsinitrd.log"); lsinitrdErr == nil {
		stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, "/tmp/lsinitrd.log", outputPath+"/lsinitrd.log")
	}

	// Best effort: capture dracut-related journal entries for initramfs boot analysis
	logrus.Tracef("Capturing dracut journal entries")
	if _, dracutErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo journalctl --no-pager -u dracut* -u systemd-udevd > /tmp/dracut-journal.log 2>&1 && sudo chmod 644 /tmp/dracut-journal.log"); dracutErr == nil {
		stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, "/tmp/dracut-journal.log", outputPath+"/dracut-journal.log")
	}

	// Download crashdumps (simplified)
	logrus.Tracef("Check for crash dumps on VM")
	crashDumpOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "ls /var/crash/*")
	if err == nil {
		logrus.Debugf("Crash files found on host: %s", crashDumpOutput)
		logrus.Error("Crash files found on host")
		stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo mv /var/crash/* /tmp/crash && sudo chmod -R 644 /tmp/crash && sudo chmod +x /tmp/crash")
		stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, "/tmp/crash/*", outputPath+"/")
	}
	return nil
}
