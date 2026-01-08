package tests

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormfile "tridenttools/storm/utils/file"
	stormsha384 "tridenttools/storm/utils/sha384"
	stormssh "tridenttools/storm/utils/ssh"
	stormtridentactivevolume "tridenttools/storm/utils/trident/activevolume"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
	"gopkg.in/yaml.v2"
)

type TestStates struct {
	InitialState   UpdateTest
	AbUpdate       UpdateTest
	RuntimeUpdate1 UpdateTest
	RuntimeUpdate2 UpdateTest
}

func createTestStates(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig, vmIP string, cosiFileName string, initialVolume string) TestStates {
	return TestStates{
		InitialState: UpdateTest{
			TestConfig:                 testConfig,
			VMConfig:                   vmConfig,
			VMIP:                       vmIP,
			CosiFileName:               cosiFileName,
			NetplanVersion:             -1,
			ExtensionVersion:           1,
			ExpectedVolume:             initialVolume,
			ExpectedAvailableRollbacks: 0,
			ExpectReboot:               false,
		},
		AbUpdate: UpdateTest{
			TestConfig:                 testConfig,
			VMConfig:                   vmConfig,
			VMIP:                       vmIP,
			CosiFileName:               cosiFileName,
			NetplanVersion:             1,
			ExtensionVersion:           2,
			ExpectedVolume:             getOtherVolume(initialVolume),
			ExpectedAvailableRollbacks: 1,
			ExpectReboot:               true,
		},
		RuntimeUpdate1: UpdateTest{
			TestConfig:                 testConfig,
			VMConfig:                   vmConfig,
			VMIP:                       vmIP,
			CosiFileName:               cosiFileName,
			NetplanVersion:             2,
			ExtensionVersion:           3,
			ExpectedVolume:             getOtherVolume(initialVolume),
			ExpectedAvailableRollbacks: 2,
			ExpectReboot:               false,
		},
		RuntimeUpdate2: UpdateTest{
			TestConfig:                 testConfig,
			VMConfig:                   vmConfig,
			VMIP:                       vmIP,
			CosiFileName:               cosiFileName,
			NetplanVersion:             -1,
			ExtensionVersion:           -1,
			ExpectedVolume:             getOtherVolume(initialVolume),
			ExpectedAvailableRollbacks: 3,
			ExpectReboot:               false,
		},
	}
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

func getOtherVolume(volume string) string {
	if volume == "volume-a" {
		return "volume-b"
	}
	return "volume-a"
}

type UpdateTest struct {
	TestConfig                 stormrollbackconfig.TestConfig
	VMConfig                   stormvmconfig.AllVMConfig
	VMIP                       string
	CosiFileName               string
	NetplanVersion             int
	ExtensionVersion           int
	ExpectedVolume             string
	ExpectedAvailableRollbacks int
	ExpectReboot               bool
}

func (u *UpdateTest) validateOs() error {
	// Verify active volume is as expected
	logrus.Tracef("Checking active volume, expecting '%s'", u.ExpectedVolume)
	checkActiveVolumeErr := stormtridentactivevolume.CheckActiveVolume(u.VMConfig.VMConfig, u.VMIP, u.ExpectedVolume)
	if checkActiveVolumeErr != nil {
		return fmt.Errorf("failed to validate active volume: %w", checkActiveVolumeErr)
	}
	if err := u.validateExtension(); err != nil {
		return fmt.Errorf("failed to validate extension: %w", err)
	}
	if err := u.validateNetplan(); err != nil {
		return fmt.Errorf("failed to validate netplan: %w", err)
	}
	return u.validateRollbacksAvailable()
}

func (u *UpdateTest) doUpdateTest() error {
	// Put Host Configuration on VM
	vmHostConfigPath := "/tmp/host_config.yaml"

	localTmpFile, err := os.CreateTemp("", "host-config-*")
	if err != nil {
		return fmt.Errorf("failed to create temporary file: %w", err)
	}
	defer localTmpFile.Close()
	hostConfig, err := u.createHostConfig()
	if err != nil {
		logrus.Tracef("Failed to create Host Configuration: %v", err)
		return fmt.Errorf("failed to create Host Configuration: %w", err)
	}
	hostConfigBytes, err := yaml.Marshal(hostConfig)
	if err != nil {
		return fmt.Errorf("failed to marshal Host Configuration: %w", err)
	}
	err = os.WriteFile(localTmpFile.Name(), hostConfigBytes, 0644)
	if err != nil {
		return fmt.Errorf("failed to write host config file locally: %w", err)
	}
	logrus.Tracef("Putting updated Host Configuration on VM:\n%s", string(hostConfigBytes))
	err = stormssh.ScpUploadFile(u.VMConfig.VMConfig, u.VMIP, localTmpFile.Name(), vmHostConfigPath)
	if err != nil {
		return fmt.Errorf("failed to put updated Host Configuration on VM (%w)", err)
	}
	logrus.Tracef("Host Configuration put on VM")
	// Invoke trident update
	logrus.Tracef("Invoking `trident update` on VM")
	updateOutput, err := stormssh.SshCommandCombinedOutput(u.VMConfig.VMConfig, u.VMIP, fmt.Sprintf("sudo trident -v trace update %s", vmHostConfigPath))
	logrus.Tracef("Update output (%v):\n%s", err, updateOutput)
	if strings.Contains(updateOutput, "Trident failed due to a servicing error") {
		return fmt.Errorf("failed to update: %w", err)
	}
	if strings.Contains(updateOutput, "No update servicing required") {
		return fmt.Errorf("no update was performed")
	}
	if !u.ExpectReboot && err != nil {
		// Ignore error from ssh if reboot was expected, but otherwise
		// an error should end the test
		return fmt.Errorf("failed to update: %w", err)
	}
	logrus.Tracef("`trident update` invoked on VM")

	if u.ExpectReboot {
		// Wait for update to complete
		logrus.Tracef("Waiting for VM to come back up after update")
		err = u.VMConfig.QemuConfig.WaitForLogin(u.VMConfig.VMConfig.Name, u.TestConfig.OutputPath, u.TestConfig.Verbose, 0)
		if err != nil {
			return fmt.Errorf("VM did not come back up after update: %w", err)
		}
		logrus.Tracef("VM ready after update")
	}

	// Check VM IP
	newVmIP, err := stormvm.GetVmIP(u.VMConfig)
	if err != nil {
		return fmt.Errorf("failed to get VM IP after update: %w", err)
	}
	if newVmIP != u.VMIP {
		return fmt.Errorf("VM IP changed after update: was '%s', now '%s'", u.VMIP, newVmIP)
	}
	logrus.Infof("VM IP remains the same after update: %s", u.VMIP)

	// Validate OS state
	err = u.validateOs()
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}
	return nil
}

func (u *UpdateTest) doRollbackTest(
	rollbackExpectation string,
	rollbackFailureExpectation string,
	rollbackNeedsReboot bool,
	needManualCommit bool,
) error {
	// Check that rollback fails if expected failure expectaqtion is set
	err := u.validateRollbackFailedExpectation(rollbackFailureExpectation)
	if err != nil {
		return fmt.Errorf("failed to validate rollback failure expectation: %w", err)
	}

	// Invoke trident rollback
	rollbackCommand := "sudo trident rollback"
	if rollbackExpectation != "" {
		rollbackCommand = fmt.Sprintf("%s %s", rollbackCommand, rollbackExpectation)
	}
	logrus.Tracef("Invoking `%s` on VM", rollbackCommand)
	updateOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, rollbackCommand)
	if !rollbackNeedsReboot && err != nil {
		// Ignore error from ssh if reboot was expected, but otherwise
		// an error should end the test
		return fmt.Errorf("failed to invoke rollback (%w):\n%s", err, updateOutput)
	}
	logrus.Tracef("`%s` invoked on VM", rollbackCommand)

	if rollbackNeedsReboot {
		// Wait for rollback to complete
		logrus.Tracef("Waiting for VM to come back up after rollback")
		err = u.VMConfig.QemuConfig.WaitForLogin(u.VMConfig.VMConfig.Name, u.TestConfig.OutputPath, u.TestConfig.Verbose, 0)
		if err != nil {
			return fmt.Errorf("VM did not come back up after rollback: %w", err)
		}
		logrus.Tracef("VM ready after rollback")
	}

	// If rolling back to initial install, trident.service is not installed, must
	// invoke commit manually
	if needManualCommit {
		logrus.Tracef("Manually invoking `trident commit` on VM")
		commitOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "sudo trident commit -v trace")
		if err != nil {
			return fmt.Errorf("failed to invoke commit (%w):\n%s", err, commitOutput)
		}
		logrus.Tracef("`trident commit` invoked on VM")
	}

	// Validate OS state
	err = u.validateOs()
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}
	return nil
}

func (u *UpdateTest) doSplitRollbackTest(
	rollbackNeedsReboot bool,
	needManualCommit bool,
) error {
	// Invoke trident rollback --allowed-operations stage
	rollbackCommand := "sudo trident rollback --allowed-operations stage"
	logrus.Tracef("Invoking `%s` on VM", rollbackCommand)
	updateOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, rollbackCommand)
	if err != nil {
		return fmt.Errorf("failed to invoke rollback (%w):\n%s", err, updateOutput)
	}
	logrus.Tracef("`%s` invoked on VM", rollbackCommand)

	// Invoke trident rollback --allowed-operations finalize
	rollbackCommand = "sudo trident rollback --allowed-operations finalize"
	logrus.Tracef("Invoking `%s` on VM", rollbackCommand)
	updateOutput, err = stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, rollbackCommand)
	if !rollbackNeedsReboot && err != nil {
		// Ignore error from ssh if reboot was expected, but otherwise
		// an error should end the test
		return fmt.Errorf("failed to invoke rollback (%w):\n%s", err, updateOutput)
	}
	logrus.Tracef("`%s` invoked on VM", rollbackCommand)

	if rollbackNeedsReboot {
		// Wait for rollback to complete
		logrus.Tracef("Waiting for VM to come back up after rollback")
		err = u.VMConfig.QemuConfig.WaitForLogin(u.VMConfig.VMConfig.Name, u.TestConfig.OutputPath, u.TestConfig.Verbose, 0)
		if err != nil {
			return fmt.Errorf("VM did not come back up after rollback: %w", err)
		}
		logrus.Tracef("VM ready after rollback")
	}

	// If rolling back to initial install, trident.service is not installed, must
	// invoke commit manually
	if needManualCommit {
		logrus.Tracef("Manually invoking `trident commit` on VM")
		commitOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "sudo trident commit -v trace")
		if err != nil {
			return fmt.Errorf("failed to invoke commit (%w):\n%s", err, commitOutput)
		}
		logrus.Tracef("`trident commit` invoked on VM")
	}

	// Validate OS state
	err = u.validateOs()
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}
	return nil
}

func (u *UpdateTest) validateRollbackFailedExpectation(
	rollbackFailureExpectation string,
) error {
	if rollbackFailureExpectation != "" {
		// Invoke trident rollback and expect failure
		logrus.Tracef("Invoking `trident rollback %s` on VM and expecting failure", rollbackFailureExpectation)
		rollbackCommand := fmt.Sprintf("sudo trident rollback %s", rollbackFailureExpectation)
		rollbackOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, rollbackCommand)
		if err == nil {
			return fmt.Errorf("expected rollback to fail but it succeeded:\n%s", rollbackOutput)
		}
		logrus.Tracef("`trident rollback %s` failed on VM as expected", rollbackFailureExpectation)
	}
	return nil
}

func (u *UpdateTest) validateRollbacksAvailable() error {
	if !u.TestConfig.SkipManualRollbacks {
		logrus.Tracef("Checking number of available rollbacks, expecting '%d'", u.ExpectedAvailableRollbacks)

		availableRollbacksOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "sudo trident get rollback-chain")
		if err != nil {
			return fmt.Errorf("'get rollback-chain' failed on VM: %v", err)
		}
		logrus.Tracef("Reported 'get rollback-chain':\n%s", availableRollbacksOutput)

		var availableRollbacks []map[string]interface{}
		err = yaml.Unmarshal([]byte(strings.TrimSpace(availableRollbacksOutput)), &availableRollbacks)
		if err != nil {
			return fmt.Errorf("failed to unmarshal available rollbacks: %w", err)
		}

		if len(availableRollbacks) != u.ExpectedAvailableRollbacks {
			return fmt.Errorf("available rollbacks mismatch: expected %d, got %d", u.ExpectedAvailableRollbacks, len(availableRollbacks))
		}
		logrus.Tracef("Available rollbacks confirmed, found: '%d'", u.ExpectedAvailableRollbacks)

		if u.ExpectedAvailableRollbacks > 0 {
			firstRollback := availableRollbacks[0]
			rollbackKind, ok := firstRollback["kind"].(string)
			if !ok {
				return fmt.Errorf("failed to parse kind from available rollback")
			}
			if (rollbackKind == "ab") != u.ExpectReboot {
				return fmt.Errorf("first available rollback kind mismatch: reboot expected: %v, got kind: %v", u.ExpectReboot, rollbackKind)
			}
			logrus.Tracef("First available rollback confirmed, found: [%s]", firstRollback)
		}

		rollbackShowValidationOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "trident rollback --check")
		if err != nil {
			return fmt.Errorf("'rollback --check' failed VM: %v", err)
		}
		logrus.Tracef("Reported 'rollback --check':\n%s", rollbackShowValidationOutput)
		if u.ExpectedAvailableRollbacks > 0 {
			if u.ExpectReboot {
				if strings.TrimSpace(rollbackShowValidationOutput) != "ab" {
					return fmt.Errorf("expected 'ab' from 'rollback --check', got: %s", rollbackShowValidationOutput)
				}
			} else {
				if strings.TrimSpace(rollbackShowValidationOutput) != "runtime" {
					return fmt.Errorf("expected 'runtime' from 'rollback --check', got: %s", rollbackShowValidationOutput)
				}
			}
		} else {
			if strings.TrimSpace(rollbackShowValidationOutput) != "none" {
				return fmt.Errorf("expected 'none' from 'rollback --check', got: %s", rollbackShowValidationOutput)
			}
		}
		logrus.Tracef("'rollback --check' output confirmed")

		rollbackShowTargetOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "sudo trident get rollback-target")
		if err != nil {
			return fmt.Errorf("'get rollback-target' failed on VM: %v", err)
		}
		logrus.Tracef("Reported 'get rollback-target':\n%s", rollbackShowTargetOutput)
		if u.ExpectedAvailableRollbacks > 0 {
			if u.ExpectReboot {
				if strings.TrimSpace(rollbackShowTargetOutput) == "{}" {
					return fmt.Errorf("expected Host Configuration from 'get rollback-target', got: %s", rollbackShowTargetOutput)
				}
			}
		} else {
			if strings.TrimSpace(rollbackShowTargetOutput) != "{}" {
				return fmt.Errorf("expected '{}' from 'get rollback-target', got: %s", rollbackShowTargetOutput)
			}
		}
		logrus.Tracef("'get rollback-target' output confirmed")
	}
	return nil
}

func (u *UpdateTest) validateExtension() error {
	if !u.TestConfig.SkipExtensionTesting {
		if u.ExtensionVersion > 0 {
			logrus.Tracef("Checking extension version, expected: '%d'", u.ExtensionVersion)
			extensionTestCommand := "test-extension.sh"
			extensionTestOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, extensionTestCommand)
			if err != nil {
				return fmt.Errorf("failed to check extension on VM (%w):\n%s", err, extensionTestOutput)
			}
			extensionTestOutput = strings.TrimSpace(extensionTestOutput)
			if extensionTestOutput != fmt.Sprintf("%d", u.ExtensionVersion) {
				return fmt.Errorf("extension version mismatch: expected %d, got %s", u.ExtensionVersion, extensionTestOutput)
			}
			logrus.Tracef("Extension version confirmed, found: '%d'", u.ExtensionVersion)
		} else {
			logrus.Tracef("Checking that extension is not present")
			extensionTestCommand := "test-extension.sh"
			extensionTestOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, extensionTestCommand)
			if err == nil {
				return fmt.Errorf("extension is unexpectedly still available:\n%s", extensionTestOutput)
			}
		}
	}
	return nil
}

func (u *UpdateTest) validateNetplan() error {
	if !u.TestConfig.SkipNetplanRuntimeTesting {
		if u.NetplanVersion > 0 {
			logrus.Tracef("Checking netplan version, expected: '%d'", u.NetplanVersion)
			netplanConfigContents, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "sudo cat /etc/netplan/99-trident.yaml")
			if err != nil {
				return fmt.Errorf("failed to read netplan config from VM image: %w", err)
			}
			logrus.Tracef("Netplan config contents:\n%s", string(netplanConfigContents))

			// Search for "dummy[0-9]:" in netplan config
			expectedDummyDevice := fmt.Sprintf("dummy%d:", u.NetplanVersion)
			regexPattern := regexp.MustCompile("dummy[0-9]:")
			matches := regexPattern.FindAllStringSubmatch(netplanConfigContents, -1)
			if len(matches) != 1 {
				return fmt.Errorf("netplan config does not contain expected dummy device")
			}
			for _, match := range matches {
				matchStr := string(match[0])
				if matchStr != expectedDummyDevice {
					return fmt.Errorf("netplan config contains unexpected version, found dummy device: %s", matchStr)
				}
			}

			// Search interfaces for expected dummy device
			interfacesOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "ip a")
			if err != nil {
				return fmt.Errorf("failed to get interfaces from VM image: %w", err)
			}
			if !strings.Contains(interfacesOutput, expectedDummyDevice) {
				return fmt.Errorf("netplan config does not contain expected dummy device in interfaces: %s", expectedDummyDevice)
			}

			logrus.Tracef("Netplan version confirmed, found: '%d'", u.NetplanVersion)
		} else {
			logrus.Tracef("Checking that netplan config is absent")
			netplanConfigContents, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "sudo cat /etc/netplan/99-trident.yaml")
			if err == nil {
				return fmt.Errorf("netplan config unexpectedly found: %s", string(netplanConfigContents))
			}

			// Search interfaces for unexpected dummy device
			interfacesOutput, err := stormssh.SshCommand(u.VMConfig.VMConfig, u.VMIP, "ip a")
			if err != nil {
				return fmt.Errorf("failed to get interfaces from VM image: %w", err)
			}
			if strings.Contains(interfacesOutput, "dummy") {
				return fmt.Errorf("dummy interface unexpectedly found: %s", string(interfacesOutput))
			}

			logrus.Tracef("Verified that netplan config is absent")
		}
	}
	return nil
}

func (u *UpdateTest) createHostConfig() (map[string]interface{}, error) {
	// Construct Host Configuration for test
	hostConfig := make(map[string]interface{})
	// Ensure OS section exists
	hostConfig["os"] = map[string]interface{}{}

	// Update Host Configuration for UpdateTest
	hostConfig["image"] = map[string]interface{}{
		"url":    fmt.Sprintf("http://localhost:%d/files/%s", u.TestConfig.FileServerPort, u.CosiFileName),
		"sha384": "ignored",
	}
	if !u.TestConfig.SkipExtensionTesting && u.ExtensionVersion > 0 {
		sysextConfig, err := u.createSysextHostConfigSection()
		if err != nil {
			return nil, fmt.Errorf("failed to create sysext host config section: %w", err)
		}
		hostConfig["os"].(map[string]interface{})["sysexts"] = sysextConfig
	}
	if !u.TestConfig.SkipNetplanRuntimeTesting && u.NetplanVersion > 0 {
		netplanConfig, err := u.createNetplanHostConfigSection()
		if err != nil {
			return nil, fmt.Errorf("failed to create netplan host config section: %w", err)
		}
		hostConfig["os"].(map[string]interface{})["netplan"] = netplanConfig
	}
	return hostConfig, nil
}

func (u *UpdateTest) createNetplanHostConfigSection() (map[string]interface{}, error) {
	dummyDevices := map[string]interface{}{}
	if u.NetplanVersion > 0 {
		dummyDevices[fmt.Sprintf("dummy%d", u.NetplanVersion)] = map[string]interface{}{
			"addresses": []string{fmt.Sprintf("192.168.%d.123/24", 100+u.NetplanVersion)},
		}
	}
	return map[string]interface{}{
		"version":       2,
		"dummy-devices": dummyDevices,
		"ethernets": map[string]interface{}{
			"vmeths": map[string]interface{}{
				"dhcp4": true,
				"match": map[string]interface{}{
					"name": "vmeth*",
				},
			},
		},
	}, nil
}

func (u *UpdateTest) createSysextHostConfigSection() ([]map[string]interface{}, error) {
	// Find existing image file
	extensionFileName := fmt.Sprintf("%s-%d.raw", u.TestConfig.ExtensionName, u.ExtensionVersion)
	extensionFile, err := stormfile.FindFile(u.TestConfig.ArtifactsDir, fmt.Sprintf("^%s$", extensionFileName))
	if err != nil {
		return nil, fmt.Errorf("failed to find extension file: %w", err)
	}
	logrus.Tracef("Found extension file: %s", extensionFile)

	// Hash the .raw file
	sha384, err := stormsha384.CalculateSha384(extensionFile)
	if err != nil {
		return nil, fmt.Errorf("failed to calculate sha384: %w", err)
	}

	return []map[string]interface{}{
		{
			"url":    fmt.Sprintf("http://localhost:%d/files/%s", u.TestConfig.FileServerPort, extensionFileName),
			"sha384": sha384,
		},
	}, nil
}
