package tests

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"
	"tridenttools/storm/servicing/utils/config"
	"tridenttools/storm/servicing/utils/file"
	"tridenttools/storm/servicing/utils/ssh"
	"tridenttools/storm/utils"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

func UpdateLoop(cfg config.ServicingConfig) error {
	return innerUpdateLoop(cfg, false)
}

func Rollback(cfg config.ServicingConfig) error {
	return innerUpdateLoop(cfg, true)
}

func innerUpdateLoop(cfg config.ServicingConfig, rollback bool) error {
	// Create context to ensure goroutines exit cleanly
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	logrus.Tracef("Stop existing update servers if any")
	// Kill any running update servers
	killUpdateServer(cfg.TestConfig.UpdatePortA)
	killUpdateServer(cfg.TestConfig.UpdatePortB)

	lsaCmd := exec.Command("ls", "-l", cfg.TestConfig.ArtifactsDir+"/update-a")
	lsaOut, err := lsaCmd.Output()
	if err != nil {
		return fmt.Errorf("failed to list update-a directory: %w", err)
	}
	logrus.Tracef("Contents of update-a directory:\n%s", lsaOut)

	lsbCmd := exec.Command("ls", "-l", cfg.TestConfig.ArtifactsDir+"/update-b")
	lsbOut, err := lsbCmd.Output()
	if err != nil {
		return fmt.Errorf("failed to list update-b directory: %w", err)
	}
	logrus.Tracef("Contents of update-b directory:\n%s", lsbOut)

	// Check for COSI files
	cosiFile, err := file.FindFile(cfg.TestConfig.ArtifactsDir+"/update-a", ".*\\.cosi$")
	if err != nil {
		return fmt.Errorf("failed to find COSI file: %w", err)
	}
	logrus.Tracef("Found COSI file: %s", cosiFile)
	cosiFileBase := cosiFile[strings.LastIndex(cosiFile, "/")+1:]

	logrus.Tracef("Start update servers (netlisten)")
	// Start update servers (netlisten)
	aStartedChannel := make(chan bool)
	go startNetListenAndWait(ctx, cfg.TestConfig.UpdatePortA, "a", cfg.TestConfig.ArtifactsDir, aStartedChannel)
	bStartedChannel := make(chan bool)
	go startNetListenAndWait(ctx, cfg.TestConfig.UpdatePortB, "b", cfg.TestConfig.ArtifactsDir, bStartedChannel)
	// Wait for both udpate servers to start
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

	vmIP, err := utils.GetVmIP(cfg)
	if err != nil {
		return fmt.Errorf("failed to get VM IP: %w", err)
	}

	// Run several commands to update/specialize update config files on VM
	logrus.Tracef("Updating config files")
	configChanges :=
		// use COSI file found in update-a and update-b directories
		fmt.Sprintf("sudo sed -i 's!verity.cosi!files/%s!' /var/lib/trident/update-config.yaml && ", cosiFileBase) +
			// use localhost as update server address
			"sudo sed -i 's/192.168.122.1/localhost/' /var/lib/trident/update-config.yaml &&" +
			// use update port a for first config (for rollback following update test, this will be no-op)
			fmt.Sprintf("sudo sed -i 's/8000/%d/' /var/lib/trident/update-config.yaml && ", cfg.TestConfig.UpdatePortA) +
			// create second config file for b update (for rollback following update test, this will align both update yamls)
			"sudo cp /var/lib/trident/update-config.yaml /var/lib/trident/update-config2.yaml && " +
			// use update port b for second config (for all cases, including rollback after update, this will set port correctly)
			fmt.Sprintf("sudo sed -i 's/%d/%d/' /var/lib/trident/update-config2.yaml", cfg.TestConfig.UpdatePortA, cfg.TestConfig.UpdatePortB)
	configChangesOutput, err := ssh.SshCommand(cfg.VMConfig, vmIP, configChanges)
	if err != nil {
		logrus.Tracef("Failed to update config files:\n%s", configChangesOutput)
		return fmt.Errorf("failed to create config for b updates")
	}

	if cfg.TestConfig.Verbose {
		configaOut, err := ssh.SshCommand(cfg.VMConfig, vmIP, "sudo cat /var/lib/trident/update-config.yaml")
		if err != nil {
			return fmt.Errorf("failed to get config a contents")
		}
		logrus.Tracef("Trident config-a contents:\n%s", configaOut)
		configbOut, err := ssh.SshCommand(cfg.VMConfig, vmIP, "sudo cat /var/lib/trident/update-config2.yaml")
		if err != nil {
			return fmt.Errorf("failed to get config b contents")
		}
		logrus.Tracef("Trident config-b contents:\n%s", configbOut)
	}

	// Main update loop (simplified)
	loopCount := cfg.TestConfig.RetryCount
	if rollback {
		loopCount = cfg.TestConfig.RollbackRetryCount
	}
	for i := 1; i <= loopCount; i++ {
		logrus.Infof("Update attempt #%d for VM '%s' (%s)", i, cfg.VMConfig.Name, cfg.VMConfig.Platform)

		if cfg.VMConfig.Platform == config.PlatformQEMU {
			if _, err := os.Stat(cfg.QemuConfig.SerialLog); err == nil {
				if err := exec.Command("truncate", "-s", "0", cfg.QemuConfig.SerialLog).Run(); err != nil {
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
				if err := cfg.QemuConfig.RebootQemuVm(cfg.VMConfig.Name, i, cfg.TestConfig.OutputPath, cfg.TestConfig.Verbose); err != nil {
					return fmt.Errorf("failed to reboot QEMU VM before update attempt #%d: %w", i, err)
				}
				if err := cfg.QemuConfig.TruncateLog(cfg.VMConfig.Name); err != nil {
					return fmt.Errorf("failed to truncate log file before update attempt #%d: %w", i, err)
				}
			}
		}

		logrus.Tracef("Setting up SSH proxy ports for update servers")
		aStartedChannel := make(chan bool)
		go ssh.StartSshProxyPortAndWait(ctx, cfg.TestConfig.UpdatePortA, vmIP, cfg.VMConfig.User, cfg.VMConfig.SshPrivateKeyPath, aStartedChannel)
		bStartedChannel := make(chan bool)
		go ssh.StartSshProxyPortAndWait(ctx, cfg.TestConfig.UpdatePortB, vmIP, cfg.VMConfig.User, cfg.VMConfig.SshPrivateKeyPath, bStartedChannel)
		// Wait for both SSH proxy ports to be ready
		<-aStartedChannel
		<-bStartedChannel

		logrus.Tracef("Checking for crash dumps on host")
		crashDumpOutput, err := ssh.SshCommand(cfg.VMConfig, vmIP, "ls /var/crash/*")
		if err == nil {
			logrus.Debugf("Crash files found on host during iteration %d: %s", i, crashDumpOutput)
			logrus.Error("Crash files found on host")
			return fmt.Errorf("crash files found on host during iteration %d", i)
		}

		if rollback && i == 1 {
			if err := prepareRollback(cfg, vmIP, updateConfig, expectedVolume, i); err != nil {
				return fmt.Errorf("failed to prepare rollback for iteration %d: %w", i, err)
			}
		}

		if cfg.TestConfig.Verbose {
			configContents, err := ssh.SshCommand(cfg.VMConfig, vmIP, fmt.Sprintf("sudo cat %s", updateConfig))
			if err != nil {
				return fmt.Errorf("failed to read update config file after modification: %w", err)
			}
			logrus.Infof("Update Config Contents:\n%s", configContents)
		}

		tridentLoggingArg := "-v WARN"
		if cfg.TestConfig.Verbose {
			tridentLoggingArg = "-v DEBUG"
		}

		logrus.Tracef("Running Trident update staging command on VM")
		combinedStagingOutput, stageErr := ssh.SshCommandCombinedOutput(cfg.VMConfig, vmIP, fmt.Sprintf("sudo trident update %s %s --allowed-operations stage", tridentLoggingArg, updateConfig))
		if cfg.TestConfig.Verbose {
			logrus.Tracef("Staging output for iteration %d:\n%s", i, combinedStagingOutput)
		}

		stageLogLocalTmpFile, err := os.CreateTemp("", "staged-trident-full")
		if err != nil {
			return fmt.Errorf("failed to create temp staging log file: %w", err)
		}
		stageLogLocalTmpPath := stageLogLocalTmpFile.Name()
		defer os.Remove(stageLogLocalTmpPath)

		err = ssh.ScpDownloadFile(cfg.VMConfig, vmIP, "/var/log/trident-full.log", stageLogLocalTmpPath)
		if err != nil {
			return fmt.Errorf("failed to download staged trident log: %w", err)
		}

		if cfg.TestConfig.OutputPath != "" {
			logrus.Tracef("Download staging trident logs for iteration %d", i)
			stageLogPath := filepath.Join(cfg.TestConfig.OutputPath, fmt.Sprintf("%s-staged-trident-full.log", fmt.Sprintf("%03d", i)))
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
		combinedFinalizeOutput, finalizeErr := ssh.SshCommandCombinedOutput(cfg.VMConfig, vmIP, fmt.Sprintf("sudo trident update %s %s --allowed-operations finalize", tridentLoggingArg, updateConfig))
		if cfg.TestConfig.Verbose {
			logrus.Tracef("Finalize output for iteration %d:\n%s\n%v", i, combinedFinalizeOutput, finalizeErr)
		}

		logrus.Tracef("Wait for VM to come back up after finalize reboot")
		if cfg.VMConfig.Platform == config.PlatformQEMU {
			err := cfg.QemuConfig.WaitForLogin(cfg.VMConfig.Name, cfg.TestConfig.OutputPath, cfg.TestConfig.Verbose, i)
			if err != nil {
				return fmt.Errorf("VM did not come back up after update for iteration %d: %w", i, err)
			}
		} else if cfg.VMConfig.Platform == config.PlatformAzure {
			time.Sleep(15 * time.Second)

			success := false
			for j := 0; j < 10; j++ {
				if _, err = ssh.SshCommand(cfg.VMConfig, vmIP, "hostname"); err == nil {
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
		newVmIP, err := utils.GetVmIP(cfg)
		if err != nil {
			return fmt.Errorf("failed to get new VM IP after update: %w", err)
		}
		if newVmIP != vmIP {
			logrus.Infof("VM IP changed from %s to %s", vmIP, newVmIP)
			return fmt.Errorf("VM IP changed during update from %s to %s", vmIP, newVmIP)
		}
		logrus.Infof("VM IP remains the same after update: %s", vmIP)

		logrus.Tracef("Validate active volume after update")
		checkActiveVolumeErr := checkActiveVolume(cfg.VMConfig, vmIP, expectedVolume)
		logrus.Tracef("Get journal logs after post-update reboot %d", i)
		if _, postUpdateJournalLogErr := ssh.SshCommand(cfg.VMConfig, vmIP, "sudo journalctl --no-pager > /tmp/post-reboot-update-journal.log && sudo chmod 644 /tmp/post-reboot-update-journal.log"); postUpdateJournalLogErr == nil {
			// Download file via scp if creating post-reboot-update-journal.log succeeded
			padIteration := fmt.Sprintf("%03d", i)
			logrus.Tracef("Downloading post-reboot-update-journal.log from VM '%s' to local machine", cfg.VMConfig.Name)
			ssh.ScpDownloadFile(cfg.VMConfig, vmIP, "/tmp/post-reboot-update-journal.log", fmt.Sprintf("%s/%s-%s", cfg.TestConfig.OutputPath, padIteration, "post-reboot-update-journal.log"))
		}
		if checkActiveVolumeErr != nil {
			return fmt.Errorf("failed to verify active volume after update: %w", checkActiveVolumeErr)
		}

		if rollback && i == 1 {
			logrus.Tracef("Validate rollback after first update")
			validateRollback(cfg.VMConfig, vmIP)
		}

		if cfg.TestConfig.Verbose {
			hostStatusStr, err := ssh.SshCommand(cfg.VMConfig, vmIP, "sudo trident get")
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

func prepareRollback(cfg config.ServicingConfig, vmIP string, updateConfig string, expectedVolume string, iteration int) error {
	logrus.Tracef("Testing Rollback for iteration %d", iteration)

	triggerRollbackScript := ".pipelines/templates/stages/testing_common/scripts/trigger-rollback.sh"
	scriptHostCopy := "/var/lib/trident/trigger-rollback.sh"

	logrus.Tracef("Copying rollback script to VM")
	if err := ssh.ScpUploadFileWithSudo(cfg.VMConfig, vmIP, triggerRollbackScript, scriptHostCopy); err != nil {
		return fmt.Errorf("failed to upload rollback script: %w", err)
	}
	logrus.Tracef("Make rollback script executable")
	if _, err := ssh.SshCommand(cfg.VMConfig, vmIP, fmt.Sprintf("sudo chmod +x %s", scriptHostCopy)); err != nil {
		return fmt.Errorf("failed to make rollback script executable: %w", err)
	}

	localConfig := "./config.yaml"
	logrus.Tracef("Downloading %s from VM to local machine: %s", updateConfig, updateConfig)
	if err := ssh.ScpDownloadFile(cfg.VMConfig, vmIP, updateConfig, localConfig); err != nil {
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
	if err := ssh.ScpUploadFileWithSudo(cfg.VMConfig, vmIP, localConfig, updateConfig); err != nil {
		return fmt.Errorf("failed to upload rollback script: %w", err)
	}
	return nil
}

func killUpdateServer(port int) error {
	logrus.Tracef("Kill process found using port %d", port)
	cmd := exec.Command("lsof", "-ti", fmt.Sprintf("tcp:%d", port))
	var out bytes.Buffer
	cmd.Stdout = &out
	if err := cmd.Run(); err != nil {
		// No process found is not an error for our use case
		logrus.Tracef("No process found on port %d", port)
		return nil
	}
	pids := strings.Fields(out.String())
	for _, pid := range pids {
		logrus.Tracef("Kill process %v", pid)
		killCmd := exec.Command("kill", "-9", pid)
		_ = killCmd.Run() // Ignore errors for robustness
	}
	return nil
}

func startNetListenAndWait(ctx context.Context, port int, partition string, artifactsDir string, startedChannel chan bool) error {
	cmdPath := "bin/netlisten"
	if _, err := os.Stat(cmdPath); os.IsNotExist(err) {
		logrus.Error("bin/netlisten not found")
		return fmt.Errorf("netlisten not found at %s: %w", cmdPath, err)
	}

	cmdArgs := []string{
		"-p", fmt.Sprint(port),
		"-s", fmt.Sprintf("%s/update-%s", artifactsDir, partition),
		"--force-color",
		"--full-logstream", fmt.Sprintf("logstream-full-update-%s.log", partition),
	}
	logrus.Tracef("netlisten started with args: %v", cmdArgs)
	cmd := exec.CommandContext(ctx, cmdPath, cmdArgs...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	if err := cmd.Start(); err != nil {
		return fmt.Errorf("failed to start netlisten for port %d: %w", port, err)
	}

	// Signal that netlisten has started
	startedChannel <- true

	// Wait for the command to finish
	if err := cmd.Wait(); err != nil {
		return fmt.Errorf("netlisten for port %d failed: %w", port, err)
	}
	logrus.Tracef("netlisten for port %d exited", port)

	return nil
}

func validateRollback(cfg config.VMConfig, vmIP string) error {
	// Get host status, but ensure this is done **after** trident.service runs
	hostStatusStr, err := ssh.SshCommand(cfg, vmIP, "set -o pipefail; sudo systemd-run --pipe --property=After=trident.service trident get")
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

func checkActiveVolume(cfg config.VMConfig, vmIP string, expectedVolume string) error {
	_, err := utils.Retry(
		time.Second*600,
		time.Second,
		func(attempt int) (*bool, error) {
			logrus.Tracef("Checking active volume (attempt %d)", attempt)
			hostStatusStr, err := ssh.SshCommandWithRetries(cfg, vmIP, "sudo trident get", 5, 5)
			if err != nil {
				return nil, fmt.Errorf("failed to get host status: %w", err)
			}
			logrus.Tracef("Retrieved host status")
			hostStatus := make(map[string]interface{})
			if err = yaml.Unmarshal([]byte(hostStatusStr), &hostStatus); err != nil {
				return nil, fmt.Errorf("failed to unmarshal YAML output: %w", err)
			}
			logrus.Tracef("Parsed host status")
			if hostStatus["servicingState"] != "provisioned" {
				return nil, fmt.Errorf("trident state is not 'provisioned'")
			}
			logrus.Tracef("Host satus servicingState is 'provisioned'")
			hsActiveVol := hostStatus["abActiveVolume"]
			if hsActiveVol != expectedVolume {
				return nil, fmt.Errorf("expected active volume '%s', got '%s'", expectedVolume, hsActiveVol)
			}
			logrus.Infof("Active volume '%s' matches expected volume '%s'", hsActiveVol, expectedVolume)
			return nil, nil
		},
	)
	return err
}
