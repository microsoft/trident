package vm

import (
	"os"
	"path/filepath"
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
	if blkidOut, blkidErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo blkid"); blkidErr == nil {
		logrus.Tracef("blkid output: %s", blkidOut)
		os.WriteFile(filepath.Join(outputPath, "blkid.log"), []byte(blkidOut), 0644)
	}

	// Best effort: capture initramfs contents to detect stale UUID references
	logrus.Tracef("Capturing lsinitrd output for initramfs diagnostics")
	if lsinitrdOut, lsinitrdErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo lsinitrd 2>/dev/null"); lsinitrdErr == nil {
		os.WriteFile(filepath.Join(outputPath, "lsinitrd.log"), []byte(lsinitrdOut), 0644)
	}

	// Best effort: capture dracut-related journal entries for initramfs boot analysis
	logrus.Tracef("Capturing dracut journal entries")
	if dracutOut, dracutErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo journalctl --no-pager -u 'dracut*' -u systemd-udevd 2>/dev/null"); dracutErr == nil {
		os.WriteFile(filepath.Join(outputPath, "dracut-journal.log"), []byte(dracutOut), 0644)
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
