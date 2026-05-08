package tests

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"
	stormsvcconfig "tridenttools/storm/servicing/utils/config"
	stormutils "tridenttools/storm/utils"
	stormfile "tridenttools/storm/utils/file"
	stormnetlisten "tridenttools/storm/utils/netlisten"
	stormssh "tridenttools/storm/utils/ssh"
	stormtridentactivevolume "tridenttools/storm/utils/trident/activevolume"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

func UpdateLoop(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return innerUpdateLoop(testConfig, vmConfig, false)
}

func Rollback(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	return innerUpdateLoop(testConfig, vmConfig, true)
}

func innerUpdateLoop(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig, rollback bool) error {
	// Create context to ensure goroutines exit cleanly
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	logrus.Tracef("Stop existing update servers if any")
	// Kill any running update servers
	stormnetlisten.KillUpdateServer(testConfig.UpdatePortA)
	stormnetlisten.KillUpdateServer(testConfig.UpdatePortB)

	lsaCmd := exec.Command("ls", "-l", testConfig.ArtifactsDir+"/update-a")
	lsaOut, err := lsaCmd.Output()
	if err != nil {
		return fmt.Errorf("failed to list update-a directory: %w", err)
	}
	logrus.Tracef("Contents of update-a directory:\n%s", lsaOut)

	lsbCmd := exec.Command("ls", "-l", testConfig.ArtifactsDir+"/update-b")
	lsbOut, err := lsbCmd.Output()
	if err != nil {
		return fmt.Errorf("failed to list update-b directory: %w", err)
	}
	logrus.Tracef("Contents of update-b directory:\n%s", lsbOut)

	// Check for COSI files
	cosiFile, err := stormfile.FindFile(testConfig.ArtifactsDir+"/update-a", ".*\\.cosi$")
	if err != nil {
		return fmt.Errorf("failed to find COSI file: %w", err)
	}
	logrus.Tracef("Found COSI file: %s", cosiFile)
	cosiFileBase := cosiFile[strings.LastIndex(cosiFile, "/")+1:]

	logrus.Tracef("Start update servers (netlisten)")
	// Start update servers (netlisten)
	aStartedChannel := make(chan bool)
	go stormnetlisten.StartNetListenAndWait(ctx, testConfig.UpdatePortA, fmt.Sprintf("%s/update-a", testConfig.ArtifactsDir), "logstream-full-update-a.log", aStartedChannel)
	bStartedChannel := make(chan bool)
	go stormnetlisten.StartNetListenAndWait(ctx, testConfig.UpdatePortB, fmt.Sprintf("%s/update-b", testConfig.ArtifactsDir), "logstream-full-update-b.log", bStartedChannel)
	// Wait for both update servers to start
	<-aStartedChannel
	<-bStartedChannel
	expectedVolume := "volume-b"
	logrus.Tracef("Current expected volume: %s", expectedVolume)

	updateConfig := "/var/lib/trident/update-config.yaml"
	if expectedVolume == "volume-a" && !rollback {
		updateConfig = "/var/lib/trident/update-config2.yaml"
	} else if expectedVolume == "volume-b" && rollback {
		updateConfig = "/var/lib/trident/update-config2.yaml"
	}
	logrus.Tracef("Using update config file: %s", updateConfig)

	vmIP, err := stormvm.GetVmIP(vmConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP: %w", err)
	}

	// Run several commands to update/specialize update config files on VM
	logrus.Tracef("Updating config files")
	configChanges :=
		// use COSI file found in update-a and update-b directories
		fmt.Sprintf("sudo sed -i 's!verity.cosi!files/%s!' /var/lib/trident/update-config.yaml && ", cosiFileBase) +
			// handle regular.cosi and verity.cosi cases
			fmt.Sprintf("sudo sed -i 's!regular.cosi!files/%s!' /var/lib/trident/update-config.yaml && ", cosiFileBase) +
			// use localhost as update server address
			"sudo sed -i 's/192.168.122.1/localhost/' /var/lib/trident/update-config.yaml &&" +
			// use update port a for first config (for rollback following update test, this will be no-op)
			fmt.Sprintf("sudo sed -i 's/8000/%d/' /var/lib/trident/update-config.yaml && ", testConfig.UpdatePortA) +
			// create second config file for b update (for rollback following update test, this will align both update yamls)
			"sudo cp /var/lib/trident/update-config.yaml /var/lib/trident/update-config2.yaml && " +
			// use update port b for second config (for all cases, including rollback after update, this will set port correctly)
			fmt.Sprintf("sudo sed -i 's/%d/%d/' /var/lib/trident/update-config2.yaml", testConfig.UpdatePortA, testConfig.UpdatePortB)
	configChangesOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, configChanges)
	if err != nil {
		logrus.Tracef("Failed to update config files:\n%s", configChangesOutput)
		return fmt.Errorf("failed to create config for b updates")
	}

	if testConfig.Verbose {
		configaOut, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo cat /var/lib/trident/update-config.yaml")
		if err != nil {
			return fmt.Errorf("failed to get config a contents")
		}
		logrus.Tracef("Trident config-a contents:\n%s", configaOut)
		configbOut, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo cat /var/lib/trident/update-config2.yaml")
		if err != nil {
			return fmt.Errorf("failed to get config b contents")
		}
		logrus.Tracef("Trident config-b contents:\n%s", configbOut)
	}

	// Main update loop (simplified)
	loopCount := testConfig.RetryCount
	if rollback {
		loopCount = testConfig.RollbackRetryCount
	}
	for i := 1; i <= loopCount; i++ {
		logrus.Infof("Update attempt #%d for VM '%s' (%s)", i, vmConfig.VMConfig.Name, vmConfig.VMConfig.Platform)

		if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
			if _, err := os.Stat(vmConfig.QemuConfig.SerialLog); err == nil {
				if err := exec.Command("truncate", "-s", "0", vmConfig.QemuConfig.SerialLog).Run(); err != nil {
					return fmt.Errorf("failed to truncate serial log file: %w", err)
				}
				dfOutput, err := exec.Command("df", "-h").Output()
				if err != nil {
					return fmt.Errorf("failed to check disk space: %w", err)
				}
				logrus.Tracef("Disk space usage:\n%s", dfOutput)
				freeOutput, err := exec.Command("free", "-h").Output()
				if err != nil {
					return fmt.Errorf("failed to check memory usage: %w", err)
				}
				logrus.Tracef("Memory usage:\n%s", freeOutput)
			}

			if i%10 == 0 {
				// For every 10th update, reboot the VM (QEMU only)
				if err := vmConfig.QemuConfig.RebootQemuVm(vmConfig.VMConfig.Name, i, testConfig.OutputPath, testConfig.Verbose); err != nil {
					return fmt.Errorf("failed to reboot QEMU VM before update attempt #%d: %w", i, err)
				}
				if err := vmConfig.QemuConfig.TruncateLog(vmConfig.VMConfig.Name); err != nil {
					return fmt.Errorf("failed to truncate log file before update attempt #%d: %w", i, err)
				}
			}
		}

		logrus.Tracef("Setting up SSH proxy ports for update servers")
		aStartedChannel := make(chan bool)
		go stormssh.StartSshProxyPortAndWait(ctx, testConfig.UpdatePortA, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, aStartedChannel)
		bStartedChannel := make(chan bool)
		go stormssh.StartSshProxyPortAndWait(ctx, testConfig.UpdatePortB, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, bStartedChannel)
		// Wait for both SSH proxy ports to be ready
		<-aStartedChannel
		<-bStartedChannel

		logrus.Tracef("Checking for crash dumps on host")
		crashDumpOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "ls /var/crash/*")
		if err == nil {
			logrus.Debugf("Crash files found on host during iteration %d: %s", i, crashDumpOutput)
			logrus.Error("Crash files found on host")
			return fmt.Errorf("crash files found on host during iteration %d", i)
		}

		if rollback && i == 1 {
			if err := prepareRollback(vmConfig, vmIP, updateConfig, expectedVolume, i); err != nil {
				return fmt.Errorf("failed to prepare rollback for iteration %d: %w", i, err)
			}
		}

		if testConfig.Verbose {
			configContents, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo cat %s", updateConfig))
			if err != nil {
				return fmt.Errorf("failed to read update config file after modification: %w", err)
			}
			logrus.Infof("Update Config Contents:\n%s", configContents)
		}

		tridentLoggingArg := "-v WARN"
		if testConfig.Verbose {
			tridentLoggingArg = "-v DEBUG"
		}

		logrus.Tracef("Running Trident update staging command on VM")
		combinedStagingOutput, stageErr := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo trident grpc-client update %s %s --allowed-operations stage", tridentLoggingArg, updateConfig))
		if testConfig.Verbose {
			logrus.Tracef("Staging output for iteration %d:\n%s", i, combinedStagingOutput)
		}

		stageLogLocalTmpFile, err := os.CreateTemp("", "staged-trident-full")
		if err != nil {
			return fmt.Errorf("failed to create temp staging log file: %w", err)
		}
		stageLogLocalTmpPath := stageLogLocalTmpFile.Name()
		defer os.Remove(stageLogLocalTmpPath)

		err = stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, "/var/log/trident-full.log", stageLogLocalTmpPath)
		if err != nil {
			return fmt.Errorf("failed to download staged trident log: %w", err)
		}

		if testConfig.OutputPath != "" {
			logrus.Tracef("Download staging trident logs for iteration %d", i)
			stageLogPath := filepath.Join(testConfig.OutputPath, fmt.Sprintf("%s-staged-trident-full.log", fmt.Sprintf("%03d", i)))
			if err := exec.Command("cp", stageLogLocalTmpPath, stageLogPath).Run(); err != nil {
				return fmt.Errorf("failed to copy staged trident log to output path: %w", err)
			}
			if err := os.Chmod(stageLogPath, 0644); err != nil {
				logrus.Errorf("failed to change permissions for staged trident log: %w", err)
			}
			if lsOut, err := exec.Command("ls", "-lh", stageLogPath).Output(); err == nil {
				logrus.Tracef("Staged trident log details for iteration %d:\n%s", i, lsOut)
			}
		}

		if stageErr != nil {
			if egrepOut, err := exec.Command("/bin/sh", "-c", fmt.Sprintf("grep 'target is busy' %s | grep umount", stageLogLocalTmpPath)).CombinedOutput(); err == nil {
				// Check for known unmount failure and signal
				logrus.Errorf("umount failure (iteration %d: %v): %s", i, stageErr, egrepOut)
				return fmt.Errorf("umount failure (iteration %d: %v)", i, stageErr)
			} else if cosiDownloadOut, err := exec.Command("/bin/sh", "-c", fmt.Sprintf("grep 'Failed to load COSI file from' %s && grep 'HTTP request failed: error sending request for url' %s", stageLogLocalTmpPath, stageLogLocalTmpPath)).CombinedOutput(); err == nil {
				// Check for known download COSI failure
				logrus.Errorf("COSI download failure (iteration %d: %v): %s", i, stageErr, cosiDownloadOut)
				return fmt.Errorf("COSI download failure (iteration %d: %v)", i, stageErr)
			}
			return fmt.Errorf("failed to stage update #%d: %w", i, stageErr)
		} else if cosiDownloadOut, err := exec.Command("/bin/sh", "-c", fmt.Sprintf("grep 'No update servicing required' %s", stageLogLocalTmpPath)).CombinedOutput(); err == nil {
			// Check for no-update-required
			logrus.Errorf("No update servicing required (iteration %d: %v): %s", i, stageErr, cosiDownloadOut)
			return fmt.Errorf("no update servicing required (iteration %d: %v)", i, stageErr)
		}

		logrus.Tracef("Running Trident update finalize command on VM")

		// Capture block device state before finalize/reboot for initramfs diagnostics.
		// If dracut embedded stale UUIDs, comparing pre-reboot blkid with post-reboot
		// lsinitrd can prove the mismatch.
		padIteration := fmt.Sprintf("%03d", i)
		if blkidOut, blkidErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo blkid"); blkidErr == nil {
			blkidPath := filepath.Join(testConfig.OutputPath, padIteration+"-pre-reboot-blkid.log")
			os.WriteFile(blkidPath, []byte(blkidOut), 0644)
		}

		combinedFinalizeOutput, finalizeErr := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo trident grpc-client update %s %s --allowed-operations finalize", tridentLoggingArg, updateConfig))
		if testConfig.Verbose {
			logrus.Tracef("Finalize output for iteration %d:\n%s\n%v", i, combinedFinalizeOutput, finalizeErr)
		}

		logrus.Tracef("Wait for VM to come back up after finalize reboot")
		if vmConfig.VMConfig.Platform == stormvmconfig.PlatformQEMU {
			err := vmConfig.QemuConfig.WaitForLogin(vmConfig.VMConfig.Name, testConfig.OutputPath, testConfig.Verbose, i)
			if err != nil {
				// Serial login detection failed. This is a known systemd/udev race
				// condition: if udev hasn't created /dev/ttyS0 when systemd evaluates
				// ConditionPathExists, dev-ttyS0.device is skipped and serial-getty
				// never starts. This happens ~2% of the time on any given boot.
				// The VM may actually be healthy — try SSH as fallback.
				logrus.Warnf("Serial login detection failed for iteration %d, attempting SSH fallback: %v", i, err)

				sshFallbackSuccess := false
				for j := 0; j < 10; j++ {
					// Check /proc/uptime to verify the VM rebooted recently (< 300s).
					// This prevents false positives from connecting to a VM that
					// never actually rebooted after the finalize step.
					output, sshErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "cat /proc/uptime")
					if sshErr == nil {
						uptimeStr := strings.Fields(strings.TrimSpace(output))
						if len(uptimeStr) > 0 {
							if uptimeSecs, parseErr := strconv.ParseFloat(uptimeStr[0], 64); parseErr == nil {
								if uptimeSecs < 300 {
									logrus.Infof("SSH fallback: VM uptime is %.1fs (recently booted)", uptimeSecs)
									sshFallbackSuccess = true
									break
								}
								logrus.Warnf("SSH fallback: VM uptime is %.1fs (too high — VM may not have rebooted)", uptimeSecs)
							}
						}
					}
					time.Sleep(3 * time.Second)
				}

				if !sshFallbackSuccess {
					// VM is genuinely unreachable — capture diagnostics
					if captureErr := stormutils.CaptureScreenshot(
						vmConfig.VMConfig.Name,
						testConfig.OutputPath,
						fmt.Sprintf("%03d-vm-failure-after-update.png", i),
					); captureErr != nil {
						logrus.Warnf("failed to capture screenshot: %v", captureErr)
					}
					checkSerialLogForDracutIssues(vmConfig.QemuConfig.SerialLog, i)
					return fmt.Errorf("VM did not come back up after update for iteration %d: %w", i, err)
				}
				logrus.Warnf("SSH fallback succeeded for iteration %d — VM is healthy but serial-getty did not start (ttyS0 device likely skipped by systemd)", i)
			}

			// Proactively ensure serial-getty@ttyS0 is running after every boot.
			// This is a no-op if it's already running, but prevents the ~2% of boots
			// where systemd skips dev-ttyS0.device (udev race) from cascading into
			// serial detection failures on subsequent iterations.
			if _, gettErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo systemctl start serial-getty@ttyS0.service"); gettErr != nil {
				logrus.Tracef("serial-getty@ttyS0 start attempt: %v (may already be running)", gettErr)
			}
		} else if vmConfig.VMConfig.Platform == stormvmconfig.PlatformAzure {
			time.Sleep(15 * time.Second)

			success := false
			for j := 0; j < 10; j++ {
				if _, err = stormssh.SshCommand(vmConfig.VMConfig, vmIP, "hostname"); err == nil {
					success = true
					break
				}
				time.Sleep(5 * time.Second) // Wait for the VM to stabilize
			}

			if !success {
				logrus.Info("Azure VM did not come back up after update")
				logrus.Errorf("Azure VM did not come back up after update for iteration %d", i)
				return fmt.Errorf("azure VM did not come back up after update for iteration %d", i)
			}
		}

		logrus.Tracef("Check if VM IP has changed after update")
		newVmIP, err := stormvm.GetVmIP(vmConfig)
		if err != nil {
			return fmt.Errorf("failed to get new VM IP after update: %w", err)
		}
		if newVmIP != vmIP {
			logrus.Infof("VM IP changed from %s to %s", vmIP, newVmIP)
			return fmt.Errorf("VM IP changed during update from %s to %s", vmIP, newVmIP)
		}
		logrus.Infof("VM IP remains the same after update: %s", vmIP)

		logrus.Tracef("Validate active volume after update")
		checkActiveVolumeErr := stormtridentactivevolume.CheckActiveVolume(vmConfig.VMConfig, vmIP, expectedVolume)
		logrus.Tracef("Get journal logs after post-update reboot %d", i)
		if _, postUpdateJournalLogErr := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo journalctl --no-pager > /tmp/post-reboot-update-journal.log && sudo chmod 644 /tmp/post-reboot-update-journal.log"); postUpdateJournalLogErr == nil {
			// Download file via scp if creating post-reboot-update-journal.log succeeded
			padIteration := fmt.Sprintf("%03d", i)
			logrus.Tracef("Downloading post-reboot-update-journal.log from VM '%s' to local machine", vmConfig.VMConfig.Name)
			stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, "/tmp/post-reboot-update-journal.log", fmt.Sprintf("%s/%s-%s", testConfig.OutputPath, padIteration, "post-reboot-update-journal.log"))
		}
		if checkActiveVolumeErr != nil {
			return fmt.Errorf("failed to verify active volume after update: %w", checkActiveVolumeErr)
		}

		if rollback && i == 1 {
			logrus.Tracef("Validate rollback after first update")
			validateRollback(vmConfig.VMConfig, vmIP)
		}

		if testConfig.Verbose {
			hostStatusStr, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo trident get")
			if err != nil {
				return fmt.Errorf("failed to get host status: %w", err)
			}
			logrus.Infof("Host Status after update:\n%s", hostStatusStr)
		}

		if expectedVolume == "volume-a" {
			expectedVolume = "volume-b"
			if !rollback || i != 1 {
				updateConfig = "/var/lib/trident/update-config.yaml"
			}
		} else {
			expectedVolume = "volume-a"
			if !rollback || i != 1 {
				updateConfig = "/var/lib/trident/update-config2.yaml"
			}
		}
		logrus.Tracef("Updated expected volume for next update to be: %s", expectedVolume)
		logrus.Tracef("Updated config file for next update to be: %s", updateConfig)
	}
	return nil
}

func prepareRollback(vmConfig stormvmconfig.AllVMConfig, vmIP string, updateConfig string, expectedVolume string, iteration int) error {
	logrus.Tracef("Testing Rollback for iteration %d", iteration)

	triggerRollbackScript := ".pipelines/templates/stages/testing_common/scripts/trigger-rollback.sh"
	scriptHostCopy := "/var/lib/trident/trigger-rollback.sh"

	logrus.Tracef("Copying rollback script to VM")
	if err := stormssh.ScpUploadFileWithSudo(vmConfig.VMConfig, vmIP, triggerRollbackScript, scriptHostCopy); err != nil {
		return fmt.Errorf("failed to upload rollback script: %w", err)
	}
	logrus.Tracef("Make rollback script executable")
	if _, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo chmod +x %s", scriptHostCopy)); err != nil {
		return fmt.Errorf("failed to make rollback script executable: %w", err)
	}

	localConfig := "./config.yaml"
	logrus.Tracef("Downloading %s from VM to local machine: %s", updateConfig, updateConfig)
	if err := stormssh.ScpDownloadFile(vmConfig.VMConfig, vmIP, updateConfig, localConfig); err != nil {
		return fmt.Errorf("failed to download update config file: %w", err)
	}

	logrus.Tracef("Add postProvision step to local config file: %s", localConfig)
	postProvisionCmd := exec.Command(
		"yq", "eval",
		".scripts.postProvision += [{\"name\": \"mount-var\", \"runOn\": [\"ab-update\"], \"content\": \"mkdir -p $TARGET_ROOT/tmp/var && mount --bind /var $TARGET_ROOT/tmp/var\"}]",
		"-i", localConfig)
	if err := postProvisionCmd.Run(); err != nil {
		return fmt.Errorf("failed to update postProvision scripts in config: %w", err)
	}

	logrus.Tracef("Add postConfigure step to invoke rollback script to local config file: %s", localConfig)
	postConfigureCmd := exec.Command(
		"yq", "eval",
		".scripts.postConfigure += [{\"name\": \"trigger-rollback\", \"runOn\": [\"ab-update\"], \"path\": \""+scriptHostCopy+"\"}]",
		"-i", localConfig)
	if err := postConfigureCmd.Run(); err != nil {
		return fmt.Errorf("failed to update postConfigure scripts in config: %w", err)
	}

	// Set writableEtcOverlayHooks flag under internalParams to true, so that the script
	// can create a new systemd service
	logrus.Tracef("Set writableEtcOverlayHooks in local config file: %s", localConfig)
	internalParamsCmd := exec.Command(
		"yq", "eval",
		".internalParams.writableEtcOverlayHooks = true",
		"-i", localConfig)
	if err := internalParamsCmd.Run(); err != nil {
		return fmt.Errorf("failed to set writableEtcOverlayHooks in config: %w", err)
	}

	logrus.Tracef("Upload modified config file to VM: %s", updateConfig)
	if err := stormssh.ScpUploadFileWithSudo(vmConfig.VMConfig, vmIP, localConfig, updateConfig); err != nil {
		return fmt.Errorf("failed to upload rollback script: %w", err)
	}
	return nil
}

func validateRollback(cfg stormvmconfig.VMConfig, vmIP string) error {
	// Get host status, but ensure this is done **after** trident.service runs
	hostStatusStr, err := stormssh.SshCommand(cfg, vmIP, "set -o pipefail; sudo systemd-run --pipe --property=After=trident.service trident get")
	if err != nil {
		return fmt.Errorf("failed to get host status: %w", err)
	}

	// Parse the host status yaml
	hostStatus := make(map[string]interface{})
	if err = yaml.Unmarshal([]byte(hostStatusStr), &hostStatus); err != nil {
		return fmt.Errorf("failed to unmarshal YAML output: %w", err)
	}

	// Validate that lastError.category is set to "servicing"
	category, ok := hostStatus["lastError"].(map[interface{}]interface{})["category"].(string)
	if ok && category != "servicing" {
		logrus.Tracef("Host status: %s", hostStatusStr)
		logrus.Errorf("Category of last error is not 'servicing', but '%s'", category)
		return fmt.Errorf("category of last error is not 'servicing', but '%s'", category)
	}

	// Validate that lastError.error contains the expected content
	error, ok := hostStatus["lastError"].(map[interface{}]interface{})["error"].(string)
	if ok && !strings.Contains(error, "!ab-update-reboot-check") {
		logrus.Errorf("Type of last error is not '!ab-update-reboot-check', but '%s'", error)
		return fmt.Errorf("type of last error is not '!ab-update-reboot-check', but '%s'", error)
	}

	// Validate that lastError.message matches the expected format
	message, ok := hostStatus["lastError"].(map[interface{}]interface{})["message"].(string)
	if ok && !regexp.MustCompile(`^A/B update failed as host booted from .+ instead of the expected device .+$`).MatchString(message) {
		logrus.Errorf("Message of last error does not match the expected format: '%s'", message)
		return fmt.Errorf("message of last error does not match the expected format: '%s'", message)
	}

	logrus.Info("Rollback validation succeeded")
	return nil
}

// checkSerialLogForDracutIssues scans the serial log for patterns that indicate
// initramfs is stuck waiting for a device, which is the symptom of bug 15086
// (stale disk UUIDs embedded in initramfs by dracut).
func checkSerialLogForDracutIssues(serialLogPath string, iteration int) {
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
		{"dracut-initqueue", "dracut-initqueue activity detected — initramfs may be waiting for a device"},
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
