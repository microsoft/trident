package tests

import (
	"context"
	"fmt"
	"strings"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormnetlisten "tridenttools/storm/utils/netlisten"
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
	err = validateOs(vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}

	logrus.Tracef("Start file server (netlisten) on test runner")
	fileServerStartedChannel := make(chan bool)
	go stormnetlisten.StartNetListenAndWait(ctx, testConfig.FileServerPort, testConfig.ArtifactsDir, "logstream-full-rollback.log", fileServerStartedChannel)
	<-fileServerStartedChannel

	// Set up SSH proxy for file server on VM
	{
		logrus.Tracef("Setting up SSH proxy ports for file server on VM")
		proxyStartedChannel := make(chan bool)
		go stormssh.StartSshProxyPortAndWait(ctx, testConfig.FileServerPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, fileServerStartedChannel)
		<-proxyStartedChannel
	}

	// Construct Host Configuration for test based on initial state
	hostConfigStr, err := stormssh.SshCommandWithRetries(vmConfig.VMConfig, vmIP, "sudo trident get configuration", 5, 5)
	if err != nil {
		return fmt.Errorf("failed to get Host Configuration: %w", err)
	}
	logrus.Tracef("Retrieved Host Configuration")
	hostConfig := make(map[string]interface{})
	if err = yaml.Unmarshal([]byte(hostConfigStr), &hostConfig); err != nil {
		return fmt.Errorf("failed to unmarshal Host Configuration: %w", err)
	}

	// Update Host Configuration for A/B update using extension version 2
	extensionVersion = 2
	hostConfig["image"] = map[string]interface{}{
		"image": map[string]interface{}{
			"url":    fmt.Sprintf("http://localhost:%d/files/regular.cosi", testConfig.FileServerPort),
			"sha384": "ignored",
		},
	}
	hostConfig["os"] = map[string]interface{}{
		"sysexts": map[string]interface{}{
			"url":    fmt.Sprintf("http://localhost:%d/files/sysext-%s%d.raw", testConfig.FileServerPort, testConfig.ExtensionName, extensionVersion),
			"sha384": "ignored",
		},
	}
	// Perform A/B update and do validation
	expectedVolume = getOtherVolume(expectedVolume)
	expectedAvailableRollbacks = 1
	err = doUpdateTest(testConfig, vmConfig, vmIP, hostConfig, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to perform first A/B update test: %w", err)
	}

	// Set up SSH proxy (again) for file server on VM after A/B update reboot
	{
		logrus.Tracef("Setting up SSH proxy ports for file server on VM")
		proxyStartedChannel := make(chan bool)
		go stormssh.StartSshProxyPortAndWait(ctx, testConfig.FileServerPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, fileServerStartedChannel)
		<-proxyStartedChannel
	}

	// Update Host Configuration for second runtime update using extension version 3
	extensionVersion = 3
	hostConfig["os"] = map[string]interface{}{
		"sysexts": map[string]interface{}{
			"url":    fmt.Sprintf("http://localhost:%d/files/sysext-%s%d.raw", testConfig.FileServerPort, testConfig.ExtensionName, extensionVersion),
			"sha384": "ignored",
		},
	}
	// Perform runtime update and do
	expectedAvailableRollbacks = 2
	err = doUpdateTest(testConfig, vmConfig, vmIP, hostConfig, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to perform first runtime update test: %w", err)
	}

	// Update Host Configuration for second runtime update removing extension
	hostConfig["os"] = map[string]interface{}{}
	// Perform runtime update and do validation
	extensionVersion = -1
	expectedAvailableRollbacks = 3
	err = doUpdateTest(testConfig, vmConfig, vmIP, hostConfig, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to perform first runtime update test: %w", err)
	}

	// Invoke rollback and expect extension 3
	extensionVersion = 3
	expectedAvailableRollbacks = 2
	err = doRollbackTest(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to perform first rollback test: %w", err)
	}

	// Invoke rollback and expect extension 2
	extensionVersion = 2
	expectedAvailableRollbacks = 1
	err = doRollbackTest(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to perform second rollback test: %w", err)
	}

	// Invoke rollback and expect extension 1
	expectedVolume = getOtherVolume(expectedVolume)
	extensionVersion = 1
	expectedAvailableRollbacks = 0
	err = doRollbackTest(testConfig, vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to perform last rollback test: %w", err)
	}

	return nil
}

func getOtherVolume(volume string) string {
	if volume == "volume-a" {
		return "volume-b"
	}
	return "volume-a"
}

func validateOs(
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
	// TODO: Verify extension
	if extensionVersion > 0 {
		logrus.Tracef("Checking extension version '%d'", extensionVersion)
		extensionTestCommand := "test-extension.sh"
		extensionTestOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, extensionTestCommand)
		if err != nil {
			return fmt.Errorf("failed to check extension on VM (%w):\n%s", err, extensionTestOutput)
		}
		if strings.TrimSpace(extensionTestOutput) != fmt.Sprintf("%d", extensionVersion) {
			return fmt.Errorf("extension version mismatch: expected %d, got %s", extensionVersion, extensionTestOutput)
		}
	} else {
		logrus.Tracef("Checking that extension is not present")
		extensionTestCommand := "test-extension.sh"
		extensionTestOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, extensionTestCommand)
		if err == nil {
			return fmt.Errorf("extension is unexpectedly still available (%w):\n%s", err, extensionTestOutput)
		}
	}
	// TODO: Verify that there is 1 available rollback
	logrus.Tracef("Checking number of available rollbacks, expecting '%d'", expectedAvailableRollbacks)
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
) error {
	// Put Host Configuration on VM
	vmHostConfigPath := "/tmp/host_config.yaml"
	hostConfigBytes, err := yaml.Marshal(hostConfig)
	if err != nil {
		return fmt.Errorf("failed to marshal Host Configuration: %w", err)
	}
	logrus.Tracef("Putting updated Host Configuration on VM")
	sshOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, fmt.Sprintf("echo '%s' | sudo tee %s", string(hostConfigBytes), vmHostConfigPath))
	if err != nil {
		return fmt.Errorf("failed to put updated Host Configuration on VM (%w):\n%s", err, sshOutput)
	}
	logrus.Tracef("Host Configuration put on VM")
	// Invoke trident update
	logrus.Tracef("Invoking `trident update` on VM")
	updateOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, fmt.Sprintf("sudo trident update -v %s", vmHostConfigPath))
	if err != nil {
		return fmt.Errorf("failed to invoke update (%w):\n%s", err, updateOutput)
	}
	logrus.Tracef("`trident update` invoked on VM")
	// Wait for update to complete
	logrus.Tracef("Waiting for VM to come back up after update")
	err = vmConfig.QemuConfig.WaitForLogin(vmConfig.VMConfig.Name, testConfig.OutputPath, testConfig.Verbose, 0)
	if err != nil {
		return fmt.Errorf("VM did not come back up after update: %w", err)
	}
	logrus.Tracef("VM ready after update")

	// Validate OS state
	err = validateOs(vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
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
) error {
	// Invoke trident rollback
	logrus.Tracef("Invoking `trident rollback` on VM")
	updateOutput, err := stormssh.SshCommand(vmConfig.VMConfig, vmIP, "sudo trident rollback")
	if err != nil {
		return fmt.Errorf("failed to invoke rollback (%w):\n%s", err, updateOutput)
	}
	logrus.Tracef("`trident rollback` invoked on VM")
	// Wait for rollback to complete
	logrus.Tracef("Waiting for VM to come back up after rollback")
	err = vmConfig.QemuConfig.WaitForLogin(vmConfig.VMConfig.Name, testConfig.OutputPath, testConfig.Verbose, 0)
	if err != nil {
		return fmt.Errorf("VM did not come back up after rollback: %w", err)
	}
	logrus.Tracef("VM ready after rollback")

	// Validate OS state
	err = validateOs(vmConfig, vmIP, extensionVersion, expectedVolume, expectedAvailableRollbacks)
	if err != nil {
		return fmt.Errorf("failed to validate OS state after update: %w", err)
	}
	return nil
}
