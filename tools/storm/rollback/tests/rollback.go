package tests

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormfile "tridenttools/storm/utils/file"
	stormnetlisten "tridenttools/storm/utils/netlisten"
	stormsha384 "tridenttools/storm/utils/sha384"
	stormssh "tridenttools/storm/utils/ssh"
	stormtridentactivevolume "tridenttools/storm/utils/trident/activevolume"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

func RollbackTest(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	// Create context to ensure goroutines exit cleanly
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	err := saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-prepare-qcow2.log")
	if err != nil {
		return fmt.Errorf("failed to save initial boot serial log: %w", err)
	}

	// Find COSI file
	cosiFile, err := stormfile.FindFile(testConfig.ArtifactsDir, ".*\\.cosi$")
	if err != nil {
		return fmt.Errorf("failed to find COSI file: %w", err)
	}
	logrus.Tracef("Found COSI file: %s", cosiFile)
	cosiFileName := filepath.Base(cosiFile)

	// Find VM IP address
	logrus.Tracef("Get VM IP after startup")
	vmIP, err := stormvm.GetVmIP(vmConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP after startup: %w", err)
	}
	logrus.Infof("VM IP remains the same after startup: %s", vmIP)

	// Validate OS state
	expectedVolume := testConfig.ExpectedVolume
	expectedAvailableRollbacks := 0
	extensionVersion := 1
	err = validateOs(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}

	logrus.Tracef("Start file server (netlisten) on test runner")
	fileServerStartedChannel := make(chan bool)
	go stormnetlisten.StartNetListenAndWait(ctx, testConfig.FileServerPort, testConfig.ArtifactsDir, "logstream-full-rollback.log", fileServerStartedChannel)
	logrus.Tracef("Waiting for file server (netlisten) to start")
	<-fileServerStartedChannel
	logrus.Tracef("File server (netlisten) started")

	// Set up SSH proxy for file server on VM
	{
		logrus.Tracef("Setting up SSH proxy ports for file server on VM")
		proxyStartedChannel := make(chan bool)
		go stormssh.StartSshProxyPortAndWait(ctx, testConfig.FileServerPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, proxyStartedChannel)
		logrus.Tracef("Waiting for SSH proxy on VM to start")
		<-proxyStartedChannel
		logrus.Tracef("SSH proxy ports for file server on VM started")
	}

	// Construct Host Configuration for test
	hostConfig := make(map[string]interface{})
	if testConfig.DebugPassword != "" {
		logrus.Tracef("Adding debug password to Host Configuration")
		hostConfig["scripts"] = map[string]interface{}{
			"postProvision": []map[string]interface{}{
				{
					"name":  "set-password-script",
					"runOn": []string{"ab-update"},
					"content": fmt.Sprintf(
						"echo '%s:%s' | sudo chpasswd",
						vmConfig.VMConfig.User,
						testConfig.DebugPassword,
					),
				},
			},
		}
	}

	// Update Host Configuration for A/B update using extension version 2
	extensionVersion = 2
	hostConfig["image"] = map[string]interface{}{
		"url":    fmt.Sprintf("http://localhost:%d/files/%s", testConfig.FileServerPort, cosiFileName),
		"sha384": "ignored",
	}
	if !testConfig.SkipExtensionTesting {
		sysextConfig, err := createSysextHostConfigSection(testConfig, vmConfig, extensionVersion)
		if err != nil {
			return fmt.Errorf("failed to create sysext host config section: %w", err)
		}
		hostConfig["os"] = sysextConfig
	}
	// Perform A/B update and do validation
	expectedVolume = getOtherVolume(expectedVolume)
	expectedAvailableRollbacks = 1
	err = doUpdateTest(testConfig, vmConfig, vmIP, hostConfig, extensionVersion, expectedVolume, expectedAvailableRollbacks, true)
	if err != nil {
		return fmt.Errorf("failed to perform first A/B update test: %w", err)
	}
	err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-ab-update.log")
	if err != nil {
		return fmt.Errorf("failed to save abupdate boot serial log: %w", err)
	}

	// Set up SSH proxy (again) for file server on VM after A/B update reboot
	{
		logrus.Tracef("Setting up SSH proxy ports for file server on VM")
		proxyStartedChannel := make(chan bool)
		go stormssh.StartSshProxyPortAndWait(ctx, testConfig.FileServerPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, proxyStartedChannel)
		<-proxyStartedChannel
	}

	if !testConfig.SkipRuntimeUpdates {
		if !testConfig.SkipExtensionTesting {
			// Update Host Configuration for second runtime update using extension version 3
			extensionVersion = 3
			sysextConfig, err := createSysextHostConfigSection(testConfig, vmConfig, extensionVersion)
			if err != nil {
				return fmt.Errorf("failed to create sysext host config section: %w", err)
			}
			hostConfig["os"] = sysextConfig
		}
		// Perform runtime update and do
		expectedAvailableRollbacks = 2
		err = doUpdateTest(testConfig, vmConfig, vmIP, hostConfig, extensionVersion, expectedVolume, expectedAvailableRollbacks, false)
		if err != nil {
			return fmt.Errorf("failed to perform first runtime update test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-runtime-update1.log")
		if err != nil {
			return fmt.Errorf("failed to save first runtime update serial log: %w", err)
		}

		// Update Host Configuration for second runtime update removing extension
		hostConfig["os"] = map[string]interface{}{}
		// Perform runtime update and do validation
		extensionVersion = -1
		expectedAvailableRollbacks = 3
		err = doUpdateTest(testConfig, vmConfig, vmIP, hostConfig, extensionVersion, expectedVolume, expectedAvailableRollbacks, false)
		if err != nil {
			return fmt.Errorf("failed to perform second runtime update test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-runtime-update2.log")
		if err != nil {
			return fmt.Errorf("failed to save second runtime update serial log: %w", err)
		}

		if !testConfig.SkipManualRollbacks {
			// Invoke rollback and expect extension 3
			extensionVersion = 3
			expectedAvailableRollbacks = 2
			err = doRollbackTest(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks, false)
			if err != nil {
				return fmt.Errorf("failed to perform first rollback test: %w", err)
			}
			err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollback1.log")
			if err != nil {
				return fmt.Errorf("failed to save first rollback serial log: %w", err)
			}

			// Invoke rollback and expect extension 2
			extensionVersion = 2
			expectedAvailableRollbacks = 1
			err = doRollbackTest(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks, false)
			if err != nil {
				return fmt.Errorf("failed to perform second rollback test: %w", err)
			}
			err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollback2.log")
			if err != nil {
				return fmt.Errorf("failed to save second rollback serial log: %w", err)
			}
		}
	}

	if !testConfig.SkipRuntimeUpdates {
		// Invoke rollback and expect extension 1
		expectedVolume = getOtherVolume(expectedVolume)
		extensionVersion = 1
		expectedAvailableRollbacks = 0
		err = doRollbackTest(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks, true)
		if err != nil {
			return fmt.Errorf("failed to perform last rollback test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollback3.log")
		if err != nil {
			return fmt.Errorf("failed to save third rollback serial log: %w", err)
		}
	}

	return nil
}

func createSysextHostConfigSection(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig, extensionVersion int) (map[string]interface{}, error) {
	// Find existing image file
	extensionFileName := fmt.Sprintf("%s-%d.raw", testConfig.ExtensionName, extensionVersion)
	extensionFile, err := stormfile.FindFile(testConfig.ArtifactsDir, fmt.Sprintf("^%s$", extensionFileName))
	if err != nil {
		return nil, fmt.Errorf("failed to find extension file: %w", err)
	}
	logrus.Tracef("Found extension file: %s", extensionFile)

	// Hash the .raw file
	sha384, err := stormsha384.CalculateSha384(extensionFile)
	if err != nil {
		return nil, fmt.Errorf("failed to calculate sha384: %w", err)
	}

	// publicKeyContents, err := os.ReadFile(fmt.Sprintf("%s.pub", vmConfig.VMConfig.SshPrivateKeyPath))
	// if err != nil {
	// 	return nil, fmt.Errorf("failed to read sysext user ssh public key: %w", err)
	// }
	return map[string]interface{}{
		// "users": []map[string]interface{}{
		// 	{
		// 		"name": vmConfig.VMConfig.User,
		// 		"sshPublicKeys": []string{
		// 			strings.TrimSpace(string(publicKeyContents)),
		// 		},
		// 		"sshMode": "key-only",
		// 	},
		// },
		"sysexts": []map[string]interface{}{
			{
				"url":    fmt.Sprintf("http://localhost:%d/files/%s", testConfig.FileServerPort, extensionFileName),
				"sha384": sha384,
			},
		},
	}, nil
}

func getOtherVolume(volume string) string {
	if volume == "volume-a" {
		return "volume-b"
	}
	return "volume-a"
}

func validateOs(
	testConfig stormrollbackconfig.TestConfig,
	vmConfig stormvmconfig.AllVMConfig,
	vmIP string,
	extensionVersion int,
	expectedVolume string,
	expectedAvailableRollbacks int,
) error {
	// Verify active volume is as expected
	logrus.Tracef("Checking active volume, expecting '%s'", expectedVolume)
	checkActiveVolumeErr := stormtridentactivevolume.CheckActiveVolume(vmConfig.VMConfig, vmIP, expectedVolume)
	if checkActiveVolumeErr != nil {
		return fmt.Errorf("failed to validate active volume: %w", checkActiveVolumeErr)
	}
	if !testConfig.SkipExtensionTesting {
		if extensionVersion > 0 {
			logrus.Tracef("Checking extension version, expected: '%d'", extensionVersion)
			extensionTestCommand := "test-extension.sh"
			extensionTestOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, extensionTestCommand)
			if err != nil {
				return fmt.Errorf("failed to check extension on VM (%w):\n%s", err, extensionTestOutput)
			}
			extensionTestOutput = strings.TrimSpace(extensionTestOutput)
			if extensionTestOutput != fmt.Sprintf("%d", extensionVersion) {
				return fmt.Errorf("extension version mismatch: expected %d, got %s", extensionVersion, extensionTestOutput)
			}
			logrus.Tracef("Extension version confirmed, found: '%d'", extensionVersion)
		} else {
			logrus.Tracef("Checking that extension is not present")
			extensionTestCommand := "test-extension.sh"
			extensionTestOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, extensionTestCommand)
			if err == nil {
				return fmt.Errorf("extension is unexpectedly still available (%w):\n%s", err, extensionTestOutput)
			}
		}
	}
	if !testConfig.SkipManualRollbacks {
		// TODO: Verify that there is 1 available rollback
		logrus.Tracef("Checking number of available rollbacks, expecting '%d'", expectedAvailableRollbacks)
	}
	return nil
}

func doUpdateTest(
	testConfig stormrollbackconfig.TestConfig,
	vmConfig stormvmconfig.AllVMConfig,
	vmIP string,
	hostConfig map[string]interface{},
	extensionVersion int,
	expectedVolume string,
	expectedAvailableRollbacks int,
	expectReboot bool,
) error {
	// Put Host Configuration on VM
	vmHostConfigPath := "/tmp/host_config.yaml"

	localTmpFile, err := os.CreateTemp("", "host-config-*")
	if err != nil {
		return fmt.Errorf("failed to create temporary file: %w", err)
	}
	defer localTmpFile.Close()
	hostConfigBytes, err := yaml.Marshal(hostConfig)
	if err != nil {
		return fmt.Errorf("failed to marshal Host Configuration: %w", err)
	}
	err = os.WriteFile(localTmpFile.Name(), hostConfigBytes, 0644)
	if err != nil {
		return fmt.Errorf("failed to write host config file locally: %w", err)
	}
	logrus.Tracef("Putting updated Host Configuration on VM")
	err = stormssh.ScpUploadFile(vmConfig.VMConfig, vmIP, localTmpFile.Name(), vmHostConfigPath)
	if err != nil {
		return fmt.Errorf("failed to put updated Host Configuration on VM (%w)", err)
	}
	logrus.Tracef("Host Configuration put on VM")
	// Invoke trident update
	logrus.Tracef("Invoking `trident update` on VM")
	updateOutput, err := stormssh.SshCommandCombinedOutput(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo trident update %s", vmHostConfigPath))
	logrus.Tracef("Update output (%v):\n%s", err, updateOutput)
	if !expectReboot && err != nil {
		// Ignore error from ssh if reboot was expected, but otherwise
		// an error should end the test
		return fmt.Errorf("failed to update: %w", err)
	}
	logrus.Tracef("`trident update` invoked on VM")

	if expectReboot {
		// Wait for update to complete
		logrus.Tracef("Waiting for VM to come back up after update")
		err = vmConfig.QemuConfig.WaitForLogin(vmConfig.VMConfig.Name, testConfig.OutputPath, testConfig.Verbose, 0)
		if err != nil {
			return fmt.Errorf("VM did not come back up after update: %w", err)
		}
		logrus.Tracef("VM ready after update")
	}

	// Check VM IP
	newVmIP, err := stormvm.GetVmIP(vmConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP after update: %w", err)
	}
	if newVmIP != vmIP {
		return fmt.Errorf("VM IP changed after update: was '%s', now '%s'", vmIP, newVmIP)
	}
	logrus.Infof("VM IP remains the same after update: %s", vmIP)

	// Validate OS state
	err = validateOs(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}
	return nil
}

func doRollbackTest(
	testConfig stormrollbackconfig.TestConfig,
	vmConfig stormvmconfig.AllVMConfig,
	vmIP string,
	extensionVersion int,
	expectedVolume string,
	expectedAvailableRollbacks int,
	expectReboot bool,
) error {
	// Invoke trident rollback
	logrus.Tracef("Invoking `trident rollback` on VM")
	updateOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo trident rollback")
	if !expectReboot && err != nil {
		// Ignore error from ssh if reboot was expected, but otherwise
		// an error should end the test
		return fmt.Errorf("failed to invoke rollback (%w):\n%s", err, updateOutput)
	}
	logrus.Tracef("`trident rollback` invoked on VM")

	if expectReboot {
		// Wait for rollback to complete
		logrus.Tracef("Waiting for VM to come back up after rollback")
		err = vmConfig.QemuConfig.WaitForLogin(vmConfig.VMConfig.Name, testConfig.OutputPath, testConfig.Verbose, 0)
		if err != nil {
			return fmt.Errorf("VM did not come back up after rollback: %w", err)
		}
		logrus.Tracef("VM ready after rollback")
	}

	// Validate OS state
	err = validateOs(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}
	return nil
}

func saveSerialAndTruncate(testConfig stormrollbackconfig.TestConfig, vmName string, serialLogFileName string) error {
	serialLogPath := filepath.Join(testConfig.OutputPath, serialLogFileName)
	logrus.Infof("Saving serial log to '%s'", serialLogPath)

	output, err := exec.Command("sudo", "cp", fmt.Sprintf("/tmp/%s.log", vmName), serialLogPath).CombinedOutput()
	logrus.Tracef("Save serial log output (%v):\n%s", err, string(output))
	if err != nil {
		return fmt.Errorf("failed to save serial log: %w", err)
	}

	logrus.Infof("Truncating serial log on QEMU VM '%s'", vmName)
	output, err = exec.Command("sudo", "truncate", "-s", "0", fmt.Sprintf("/tmp/%s.log", vmName)).CombinedOutput()
	logrus.Tracef("Truncate serial log output (%v):\n%s", err, string(output))
	if err != nil {
		return fmt.Errorf("failed to truncate serial log: %w", err)
	}
	return nil
}
