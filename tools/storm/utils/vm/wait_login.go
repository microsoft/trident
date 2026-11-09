package vm

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	stormutils "tridenttools/storm/utils"
	stormssh "tridenttools/storm/utils/ssh"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

// WaitForLoginWithSshFallback waits for the VM to come back up after a reboot.
//
// On QEMU it waits for the serial "login:" prompt; if serial login detection
// times out it falls back to confirming the VM rebooted via SSH
// ("uptime --since"), tolerating known flaky-boot races such as the
// serial-getty udev race (systemd#10850) and the intermittent Azure Linux 4
// generator-sandbox freeze that triggers a watchdog reset + reboot. On Azure it
// polls SSH for liveness.
//
// preRebootUptime is the "uptime --since" value captured before the reboot was
// triggered; when non-empty it is used to confirm the value changed (proving a
// reboot occurred). Pass "" to accept any SSH reachability as success.
//
// On QEMU it also proactively (re)starts serial-getty@ttyS0 so subsequent boots
// are detected via the serial log. On genuine failure it captures a screenshot
// and scans the serial log for dracut/initramfs issues.
func WaitForLoginWithSshFallback(vmConfig stormvmconfig.AllVMConfig, vmIP string, preRebootUptime string, iteration int, outputPath string, verbose bool) error {
	if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
		time.Sleep(15 * time.Second)
		for j := 0; j < 10; j++ {
			if _, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "hostname"); err == nil {
				return nil
			}
			time.Sleep(5 * time.Second) // Wait for the VM to stabilize
		}
		return fmt.Errorf("azure VM did not come back up after reboot for iteration %d", iteration)
	}

	err := vmConfig.QemuConfig.WaitForLogin(vmConfig.VMConfig.Name, outputPath, verbose, iteration)
	if err != nil {
		// Serial login detection failed — the "login:" prompt did not appear in
		// the serial log within the timeout period.
		//
		// Known causes:
		//   - serial-getty@ttyS0.service depends on the auto-generated
		//     dev-ttyS0.device unit; if udev is slow creating /dev/ttyS0,
		//     systemd's ConditionPathExists check fails and serial-getty never
		//     starts, so no "login:" appears even though the VM is healthy
		//     (systemd#10850, ~2% of boots).
		//   - On Azure Linux 4, systemd's manager startup intermittently fails
		//     to fork its generator sandbox (EPROTO) and freezes PID1; the UEFI
		//     watchdog then resets the VM, which reboots and recovers. That
		//     freeze+reset+reboot cycle can exceed the serial wait window.
		//
		// Fallback: confirm the VM rebooted by comparing "uptime --since" before
		// and after the reboot. If the value changed (or we have no baseline and
		// SSH is reachable), the VM is confirmed alive and we can proceed.
		logrus.Warnf("Serial login detection failed for iteration %d, attempting SSH fallback: %v", iteration, err)

		sshFallbackSuccess := false
		for j := 0; j < 10; j++ {
			output, sshErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "uptime --since")
			if sshErr == nil {
				postRebootUptime := strings.TrimSpace(output)
				if preRebootUptime != "" && postRebootUptime != preRebootUptime {
					logrus.Infof("SSH fallback: VM rebooted (uptime --since changed from %q to %q)", preRebootUptime, postRebootUptime)
					sshFallbackSuccess = true
					break
				} else if preRebootUptime == "" {
					// No pre-reboot uptime captured — accept any SSH response
					logrus.Infof("SSH fallback: VM reachable via SSH (uptime --since: %s, no pre-reboot baseline)", postRebootUptime)
					sshFallbackSuccess = true
					break
				}
				logrus.Warnf("SSH fallback: uptime --since unchanged (%q) — VM may not have rebooted yet", postRebootUptime)
			}
			time.Sleep(3 * time.Second)
		}

		if !sshFallbackSuccess {
			// VM is genuinely unreachable — capture diagnostics
			if captureErr := stormutils.CaptureScreenshot(
				vmConfig.VMConfig.Name,
				outputPath,
				fmt.Sprintf("%03d-vm-failure-after-reboot.png", iteration),
			); captureErr != nil {
				logrus.Warnf("failed to capture screenshot: %v", captureErr)
			}
			// Check serial log for dracut-initqueue timeout patterns that indicate
			// stale disk UUIDs in initramfs (see bug 15086). Use the saved copy
			// since WaitForLogin truncates the original.
			if outputPath != "" {
				savedSerialLog := filepath.Join(outputPath, fmt.Sprintf("%03d-serial.log", iteration))
				CheckSerialLogForDracutIssues(savedSerialLog, iteration)
			}
			return fmt.Errorf("VM did not come back up after reboot for iteration %d: %w", iteration, err)
		}
		logrus.Warnf("SSH fallback succeeded for iteration %d — VM is healthy but serial-getty did not start (ttyS0 device likely skipped by systemd)", iteration)
	}

	// Proactively ensure serial-getty@ttyS0 is running after every boot. This is
	// a no-op if already running. When systemd skips dev-ttyS0.device due to the
	// udev race condition (systemd#10850), this restarts serial-getty so
	// subsequent iterations detect "login:" normally via the serial log.
	if _, gettErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo systemctl start serial-getty@ttyS0.service"); gettErr != nil {
		logrus.Tracef("serial-getty@ttyS0 start attempt: %v (may already be running)", gettErr)
	}
	return nil
}

// CheckSerialLogForDracutIssues scans a saved serial log for dracut/initramfs
// failure patterns (e.g. stale UUIDs in initramfs, bug 15086) and logs a
// diagnostic for each match.
func CheckSerialLogForDracutIssues(serialLogPath string, iteration int) {
	if serialLogPath == "" {
		return
	}
	data, err := os.ReadFile(serialLogPath)
	if err != nil {
		logrus.Warnf("Could not read serial log for dracut analysis: %v", err)
		return
	}
	content := string(data)

	dracutPatterns := []struct {
		pattern string
		message string
	}{
		{"dracut-initqueue[", "dracut-initqueue warning detected — initramfs may be waiting for a device"},
		{"Could not boot", "dracut 'Could not boot' error detected"},
		{"Starting dracut emergency shell", "dracut emergency shell activated — boot failed in initramfs"},
		{"Warning: /dev/disk/by", "dracut warning about /dev/disk/by-* path — possible stale UUID reference"},
		{"rd.break", "rd.break detected — initramfs dropped to debug shell"},
		{"Timed out waiting for device", "dracut timed out waiting for device — likely stale UUID in initramfs (bug 15086)"},
	}

	for _, dp := range dracutPatterns {
		if strings.Contains(content, dp.pattern) {
			logrus.Errorf("INITRAMFS DIAGNOSTIC (iteration %d): %s (matched '%s' in serial log)", iteration, dp.message, dp.pattern)
		}
	}
}
