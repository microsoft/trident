package tests

import (
	"context"
	"fmt"
	"path/filepath"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormfile "tridenttools/storm/utils/file"
	stormnetlisten "tridenttools/storm/utils/netlisten"
	stormssh "tridenttools/storm/utils/ssh"
	stormvm "tridenttools/storm/utils/vm"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

func MultiRollbackTest(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
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

	// Create test states
	testStates := createTestStates(testConfig, vmConfig, vmIP, cosiFileName, testConfig.ExpectedVolume)

	// Validate OS state
	err = testStates.InitialState.validateOs()
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

	// Perform A/B update and do validation
	err = testStates.AbUpdate.doUpdateTest()
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
		// Perform runtime update and do validation
		err = testStates.RuntimeUpdate1.doUpdateTest()
		if err != nil {
			return fmt.Errorf("failed to perform first runtime update test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-runtime-update1.log")
		if err != nil {
			return fmt.Errorf("failed to save first runtime update serial log: %w", err)
		}

		// Update Host Configuration for second runtime update removing extension
		err = testStates.RuntimeUpdate2.doUpdateTest()
		if err != nil {
			return fmt.Errorf("failed to perform second runtime update test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-runtime-update2.log")
		if err != nil {
			return fmt.Errorf("failed to save second runtime update serial log: %w", err)
		}

		if !testConfig.SkipManualRollbacks {
			// Invoke `rollback --runtime` of runtime update 2 into state after runtime update 1
			err = testStates.RuntimeUpdate1.doRollbackTest("--runtime", "", testStates.RuntimeUpdate2.ExpectReboot, false)
			if err != nil {
				return fmt.Errorf("failed to perform first rollback test: %w", err)
			}
			err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollback1.log")
			if err != nil {
				return fmt.Errorf("failed to save first rollback serial log: %w", err)
			}

			// Invoke `rollback` of runtime update 1 into state after ab update
			err = testStates.AbUpdate.doRollbackTest("", "", testStates.RuntimeUpdate1.ExpectReboot, false)
			if err != nil {
				return fmt.Errorf("failed to perform second rollback test: %w", err)
			}
			err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollback2.log")
			if err != nil {
				return fmt.Errorf("failed to save second rollback serial log: %w", err)
			}
		}
	}

	if !testConfig.SkipManualRollbacks {
		// Invoke `rollback` of ab update into initial state, also validate that
		// `rollback --runtime` fails when the next rollback is for an A/B update.
		err = testStates.InitialState.doRollbackTest("", "--runtime", testStates.AbUpdate.ExpectReboot, true)
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

func SkipToAbRollbackTest(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	// Check `rollback --ab`, need to:
	//   1. abupdate
	//   2. runtime update
	//   3. rollback --ab
	if !testConfig.SkipRuntimeUpdates && !testConfig.SkipManualRollbacks {
		// Create context to ensure goroutines exit cleanly
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()

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

		// Create test states
		testStates := createTestStates(testConfig, vmConfig, vmIP, cosiFileName, testConfig.ExpectedVolume)

		// Validate OS state
		err = testStates.InitialState.validateOs()
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

		// Perform A/B update
		err = testStates.AbUpdate.doUpdateTest()
		if err != nil {
			return fmt.Errorf("failed to perform A/B update for rollback-ab test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollbackab-ab-update.log")
		if err != nil {
			return fmt.Errorf("failed to save rollback-ab A/B update boot serial log: %w", err)
		}

		// Set up SSH proxy (again) for file server on VM after A/B update reboot
		{
			logrus.Tracef("Setting up SSH proxy ports for file server on VM")
			proxyStartedChannel := make(chan bool)
			go stormssh.StartSshProxyPortAndWait(ctx, testConfig.FileServerPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, proxyStartedChannel)
			<-proxyStartedChannel
		}

		// Perform runtime update
		err = testStates.RuntimeUpdate1.doUpdateTest()
		if err != nil {
			return fmt.Errorf("failed to perform first runtime update for rollback-ab test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollbackab-runtime-update.log")
		if err != nil {
			return fmt.Errorf("failed to save first runtime update serial log for rollback-ab test: %w", err)
		}

		// Invoke `rollback --ab` of ab update into initial state
		err = testStates.InitialState.doRollbackTest("--ab", "", testStates.AbUpdate.ExpectReboot, true)
		if err != nil {
			return fmt.Errorf("failed to perform `rollback --ab` for rollback-ab test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-rollbackab-rollback.log")
		if err != nil {
			return fmt.Errorf("failed to save `rollback --ab` serial log for rollback-ab test: %w", err)
		}
	}

	return nil
}

func SplitRollbackTest(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	// Check `rollback --allowed-operations` for ab update rollback, need to:
	//   1. abupdate
	//   2. runtime update
	//   3. (for runtime udpate) rollback --allowed-operations stage
	//   4. (for runtime udpate) rollback --allowed-operations finalize
	//   5. (for ab udpate) rollback --allowed-operations stage
	//   6. (for ab udpate) rollback --allowed-operations finalize
	if !testConfig.SkipManualRollbacks {
		// Create context to ensure goroutines exit cleanly
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()

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

		// Create test states
		testStates := createTestStates(testConfig, vmConfig, vmIP, cosiFileName, testConfig.ExpectedVolume)

		// Validate OS state
		err = testStates.InitialState.validateOs()
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

		// Perform A/B update
		err = testStates.AbUpdate.doUpdateTest()
		if err != nil {
			return fmt.Errorf("failed to perform split test A/B update: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-split-ab-update-1.log")
		if err != nil {
			return fmt.Errorf("failed to save split test A/B update boot serial log: %w", err)
		}

		// Set up SSH proxy (again) for file server on VM after A/B update reboot
		{
			logrus.Tracef("Setting up SSH proxy ports for file server on VM")
			proxyStartedChannel := make(chan bool)
			go stormssh.StartSshProxyPortAndWait(ctx, testConfig.FileServerPort, vmIP, vmConfig.VMConfig.User, vmConfig.VMConfig.SshPrivateKeyPath, proxyStartedChannel)
			<-proxyStartedChannel
		}

		// Perform runtime update
		err = testStates.RuntimeUpdate1.doUpdateTest()
		if err != nil {
			return fmt.Errorf("failed to perform split test runtime update test: %w", err)
		}

		// Invoke `rollback` using allowed-operations of runtime update into ab update state
		err = testStates.AbUpdate.doSplitRollbackTest(testStates.RuntimeUpdate1.ExpectReboot, true)
		if err != nil {
			return fmt.Errorf("failed to perform `rollback` split test: %w", err)
		}

		// Invoke `rollback` using allowed-operations of ab update into initial state
		err = testStates.InitialState.doSplitRollbackTest(testStates.AbUpdate.ExpectReboot, true)
		if err != nil {
			return fmt.Errorf("failed to perform `rollback` split test: %w", err)
		}
		err = saveSerialAndTruncate(testConfig, vmConfig.VMConfig.Name, "serial-split-rollback-ab.log")
		if err != nil {
			return fmt.Errorf("failed to save `rollback` split serial log: %w", err)
		}
	}

	return nil
}
