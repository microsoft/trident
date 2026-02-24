package scenario

import (
	"context"
	"fmt"
	"net/http"
	"path"
	"regexp"
	"strings"
	"time"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"

	"tridenttools/pkg/hostconfig"
	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/netlisten"
	"tridenttools/storm/e2e/testrings"
	"tridenttools/storm/utils/ssh/sftp"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/trident"
)

const (
	hostConfigRemotePath = "/var/lib/trident/config.yaml"
)

// addAbUpdateTests adds the A/B update test cases to the provided test registrar
func (s *TridentE2EScenario) addAbUpdateTests(r storm.TestRegistrar, prefix string) {
	r.RegisterTestCase(prefix+"-sync-hc", s.syncHostConfig)
	r.RegisterTestCase(prefix+"-update-hc", s.updateHostConfig)
	r.RegisterTestCase(prefix+"-upload-new-hc", s.uploadNewConfig)
	r.RegisterTestCase(prefix+"-ab-update", func(tc storm.TestCase) error {
		return s.abUpdateOs(tc, false)
	})
}

// addSplitABUpdateTests adds the split A/B update test cases to the provided test registrar
func (s *TridentE2EScenario) addSplitABUpdateTests(r storm.TestRegistrar, prefix string) {
	filterSplitTestForCurrentRing := func(s *TridentE2EScenario, tc storm.TestCase, testFn func(storm.TestCase) error) error {
		// The lowest ring for which we do split testing is 'prerelease'.
		if s.args.TestRing < testrings.TestRingPre {
			tc.Skip(fmt.Sprintf("Skipping split AB update test on ring '%s'", s.args.TestRing.ToString()))
		}

		return testFn(tc)
	}

	r.RegisterTestCase(prefix+"-sync-hc", func(tc storm.TestCase) error {
		return filterSplitTestForCurrentRing(s, tc, s.syncHostConfig)
	})
	r.RegisterTestCase(prefix+"-update-hc", func(tc storm.TestCase) error {
		return filterSplitTestForCurrentRing(s, tc, s.updateHostConfig)
	})
	r.RegisterTestCase(prefix+"-upload-new-hc", func(tc storm.TestCase) error {
		return filterSplitTestForCurrentRing(s, tc, s.uploadNewConfig)
	})
	r.RegisterTestCase(prefix+"-stage", func(tc storm.TestCase) error {
		return filterSplitTestForCurrentRing(s, tc, s.abStageOs)
	})
	r.RegisterTestCase(prefix+"-validate-staged", func(tc storm.TestCase) error {
		return filterSplitTestForCurrentRing(s, tc, s.validateAbStaged)
	})
	r.RegisterTestCase(prefix+"-finalize", func(tc storm.TestCase) error {
		return filterSplitTestForCurrentRing(s, tc, s.abFinalizeOs)
	})
}

func (s *TridentE2EScenario) syncHostConfig(tc storm.TestCase) error {
	// ensure ssh client is populated
	err := s.populateSshClient(tc.Context())
	if err != nil {
		// At this point we know the VM is up, so failing to populate SSH client is a test error.
		return fmt.Errorf("failed to populate SSH client: %w", err)
	}

	out, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "get configuration")
	if err != nil {
		return fmt.Errorf("failed to get host configuration via Trident: %w", err)
	}

	s.config, err = hostconfig.NewHostConfigFromYaml([]byte(out.Stdout))
	if err != nil {
		return fmt.Errorf("failed to parse host configuration from Trident output: %w", err)
	}

	return nil
}

func (s *TridentE2EScenario) updateHostConfig(tc storm.TestCase) error {
	// Bump the image version by 1:
	s.version += 1

	// Get the old image URL from config
	oldUrl, ok := s.config.S("image", "url").Data().(string)
	if !ok {
		return fmt.Errorf("failed to get old image URL from config")
	}

	logrus.Infof("Old image URL: %s", oldUrl)

	// Extract the base name of the image URL
	base := path.Base(oldUrl)
	if base == "" {
		return fmt.Errorf("failed to get base name from URL: %s", oldUrl)
	}

	// Get the URL path without the base name
	urlPath, ok := strings.CutSuffix(oldUrl, base)
	if !ok {
		return fmt.Errorf("failed to remove suffix '%s' from URL '%s'", base, oldUrl)
	}

	logrus.Debugf("Base name: %s", base)

	var newCosiName string
	if strings.HasPrefix(oldUrl, "oci://") {
		// Special handling for OCI URLs

		// Match form <repository_base>:v<build ID>.<config>.<deployment env>.<version number>
		matches := regexp.MustCompile(`^(.+):v(\d+)\.(.+)\.(.+)\.(\d+)$`).FindStringSubmatch(base)
		if len(matches) != 6 {
			return fmt.Errorf("failed to parse OCI image name: %s", base)
		}

		name := matches[1]
		buildId := matches[2]
		configName := matches[3]
		deploymentEnv := matches[4]
		newCosiName = fmt.Sprintf("%s:v%s.%s.%s.%d", name, buildId, configName, deploymentEnv, s.version)
	} else {
		// Match form <name>_v<version number>.<file extension> (note that "_v<version number>" is optional)
		matches := regexp.MustCompile(`^(.*?)(_v\d+)?\.(.+)$`).FindStringSubmatch(base)
		if len(matches) != 4 {
			return fmt.Errorf("failed to parse image name: %s", base)
		}

		name := matches[1]
		ext := matches[3]
		newCosiName = fmt.Sprintf("%s_v%d.%s", name, s.version, ext)
	}

	newUrl := fmt.Sprintf("%s%s", urlPath, newCosiName)
	logrus.Infof("New image URL: %s", newUrl)

	logrus.Infof("Checking if new image URL is accessible...")
	err := checkUrlIsAccessible(newUrl)
	if err != nil {
		logrus.WithError(err).Errorf("New image URL is not accessible: %s (continuing)", newUrl)
	} else {
		logrus.Infof("New image URL is accessible")
	}

	// Update the config with the new image URL and ignore the SHA384 checksum, and BLOCK self-upgrade.
	s.config.Set(newUrl, "image", "url")
	s.config.Set("ignored", "image", "sha384")
	s.config.Set(false, "internalParams", "selfUpgradeTrident")
	// Remove storage section which is not needed for AB update.
	s.config.Delete("storage")

	return nil
}

func (s *TridentE2EScenario) uploadNewConfig(tc storm.TestCase) error {
	// ensure ssh client is populated
	err := s.populateSshClient(tc.Context())
	if err != nil {
		// At this point we know the VM is up, so failing to populate SSH client is a test error.
		return fmt.Errorf("failed to populate SSH client: %w", err)
	}

	sftpClient, err := sftp.NewSftpSudoClient(s.sshClient)
	if err != nil {
		return fmt.Errorf("failed to create SFTP sudo client: %w", err)
	}
	defer sftpClient.Close()

	// Write the updated host config to /tmp/host_config.yaml on the test host
	hostConfigFile, err := s.config.ToYaml()
	if err != nil {
		return fmt.Errorf("failed to render host configuration: %w", err)
	}

	remoteFile, err := sftpClient.Create(hostConfigRemotePath)
	if err != nil {
		return fmt.Errorf("failed to create remote host config file '%s': %w", hostConfigRemotePath, err)
	}
	defer remoteFile.Close()

	_, err = remoteFile.Write(hostConfigFile)
	if err != nil {
		remoteFile.Close()
		return fmt.Errorf("failed to write to remote host config file '%s': %w", hostConfigRemotePath, err)
	}

	err = remoteFile.Chmod(0644)
	if err != nil {
		return fmt.Errorf("failed to change permissions of new Host Config file: %w", err)
	}

	err = remoteFile.Chown(0, 0)
	if err != nil {
		return fmt.Errorf("failed to change ownership of new Host Config file: %w", err)
	}

	return nil
}

func (s *TridentE2EScenario) abUpdateOs(tc storm.TestCase, split bool) error {
	args := fmt.Sprintf(
		"update -v trace %s",
		path.Join(s.runtime.HostPath(), hostConfigRemotePath),
	)

	// Get the Host Config file to be used for the update, for debugging purposes
	file, err := sshutils.CommandOutput(s.sshClient, fmt.Sprintf("sudo cat %s", hostConfigRemotePath))
	if err != nil {
		return fmt.Errorf("failed to read new Host Config file: %w", err)
	}

	logrus.Debugf("Trident HC file @ %s:\n%s", hostConfigRemotePath, file)

	go netlisten.RunNetlisten(tc.Context(), &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort:           defaultNetlaunchListenPort,
			LogstreamFile:        s.args.LogstreamFile,
			TracestreamFile:      fmt.Sprintf("metrics-%s.jsonl", tc.Name()),
			ServeDirectory:       s.args.TestImageDir,
			MaxPhonehomeFailures: s.configParams.MaxExpectedFailures,
		},
	})

	monitorCtx, cancel := context.WithCancel(tc.Context())
	defer cancel()

	// Start VM serial monitor (only runs if hardware is VM)
	monWaitChan, monErr := s.spawnVMSerialMonitor(monitorCtx, tc.ArtifactBroker().StreamArtifactData(tc.Name()+"/serial.log"))
	if monErr != nil {
		return fmt.Errorf("failed to start VM serial monitor: %w", monErr)
	}

	// On exit, give the monitor up to 1 minute to reach the login prompt and exit.
	defer func() {
		select {
		case <-time.After(time.Minute):
			logrus.Infof("Waited 1 minute for serial monitor to reach login prompt, cancelling monitor.")
			cancel()
		case <-monWaitChan:
			// Monitor exited on its own
		}
	}()

	if !split {
		// regular case
		logrus.Infof("Running Trident A/B update...")
		err = runTridentUpdate(tc, s.runtime, s.sshClient, args)
		if err != nil {
			return fmt.Errorf("failed to run Trident A/B update: %w", err)
		}
	} else {
		// split stage and finalize
		logrus.Infof("Running split Trident A/B update (stage)...")
		err = runTridentUpdate(tc, s.runtime, s.sshClient, args+" --allowed-operations stage")
		if err != nil {
			return fmt.Errorf("failed to run Trident A/B update: %w", err)
		}

		logrus.Infof("Running split Trident A/B update (finalize)...")
		err = runTridentUpdate(tc, s.runtime, s.sshClient, args+" --allowed-operations finalize")
		if err != nil {
			return fmt.Errorf("failed to run Trident A/B update: %w", err)
		}
	}

	// Wait for SSH client to disconnect, meaning the host is rebooting, before
	// trying to reconnect again.
	logrus.Info("Waiting for SSH client to disconnect after Trident A/B update...")
	disconnectCtx, cancel := context.WithTimeout(tc.Context(), time.Minute*2)
	defer cancel()
	err = s.waitForSshToDisconnect(disconnectCtx)
	if err != nil {
		// At this point we expect the host to be rebooting, so failure to detect
		// disconnection is a test failure.
		tc.FailFromError(fmt.Errorf("failed to detect SSH disconnection after Trident A/B update: %w", err))
	}

	logrus.Info("SSH client disconnected, host is rebooting. Will attempt to reconnect...")

	// Then, try to reconnect via SSH and check that Trident is running.
	// Longer timeout since the host will be rebooting while we wait.
	conn_ctx, cancel := context.WithTimeout(tc.Context(), time.Minute*5)
	defer cancel()
	err = s.populateSshClient(conn_ctx)
	if err != nil {
		tc.FailFromError(err)
		return nil
	}

	logrus.Info("Reacquired SSH connection to host after reboot.")

	// Give it some extra time to ensure Trident is up after reboot.
	err = trident.CheckTridentService(s.sshClient, s.runtime, time.Minute*2, true)
	if err != nil {
		tc.FailFromError(err)
	}

	return nil
}

// abStageOs stages an A/B update on the test host without finalizing.
// This is used in the split A/B update flow where staging and finalization
// are separate test steps with a validation check in between.
func (s *TridentE2EScenario) abStageOs(tc storm.TestCase) error {
	args := fmt.Sprintf(
		"update -v trace %s",
		path.Join(s.runtime.HostPath(), hostConfigRemotePath),
	)

	// Get the Host Config file to be used for the update, for debugging purposes
	file, err := sshutils.CommandOutput(s.sshClient, fmt.Sprintf("sudo cat %s", hostConfigRemotePath))
	if err != nil {
		return fmt.Errorf("failed to read new Host Config file: %w", err)
	}

	logrus.Debugf("Trident HC file @ %s:\n%s", hostConfigRemotePath, file)

	go netlisten.RunNetlisten(tc.Context(), &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort:           defaultNetlaunchListenPort,
			LogstreamFile:        s.args.LogstreamFile,
			TracestreamFile:      fmt.Sprintf("metrics-%s.jsonl", tc.Name()),
			ServeDirectory:       s.args.TestImageDir,
			MaxPhonehomeFailures: s.configParams.MaxExpectedFailures,
		},
	})

	logrus.Infof("Running split Trident A/B update (stage)...")
	err = runTridentUpdate(tc, s.runtime, s.sshClient, args+" --allowed-operations stage")
	if err != nil {
		return fmt.Errorf("failed to run Trident A/B update (stage): %w", err)
	}

	return nil
}

// abFinalizeOs finalizes a previously staged A/B update, handling reboot
// and SSH reconnection. This is used in the split A/B update flow.
func (s *TridentE2EScenario) abFinalizeOs(tc storm.TestCase) error {
	args := fmt.Sprintf(
		"update -v trace %s",
		path.Join(s.runtime.HostPath(), hostConfigRemotePath),
	)

	go netlisten.RunNetlisten(tc.Context(), &netlaunch.NetListenConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort:           defaultNetlaunchListenPort,
			LogstreamFile:        s.args.LogstreamFile,
			TracestreamFile:      fmt.Sprintf("metrics-%s.jsonl", tc.Name()),
			ServeDirectory:       s.args.TestImageDir,
			MaxPhonehomeFailures: s.configParams.MaxExpectedFailures,
		},
	})

	monitorCtx, cancel := context.WithCancel(tc.Context())
	defer cancel()

	// Start VM serial monitor (only runs if hardware is VM)
	monWaitChan, monErr := s.spawnVMSerialMonitor(monitorCtx, tc.ArtifactBroker().StreamArtifactData(tc.Name()+"/serial.log"))
	if monErr != nil {
		return fmt.Errorf("failed to start VM serial monitor: %w", monErr)
	}

	// On exit, give the monitor up to 1 minute to reach the login prompt and exit.
	defer func() {
		select {
		case <-time.After(time.Minute):
			logrus.Infof("Waited 1 minute for serial monitor to reach login prompt, cancelling monitor.")
			cancel()
		case <-monWaitChan:
			// Monitor exited on its own
		}
	}()

	logrus.Infof("Running split Trident A/B update (finalize)...")
	err := runTridentUpdate(tc, s.runtime, s.sshClient, args+" --allowed-operations finalize")
	if err != nil {
		return fmt.Errorf("failed to run Trident A/B update (finalize): %w", err)
	}

	// Wait for SSH client to disconnect, meaning the host is rebooting, before
	// trying to reconnect again.
	logrus.Info("Waiting for SSH client to disconnect after Trident A/B update...")
	disconnectCtx, disconnectCancel := context.WithTimeout(tc.Context(), time.Minute*2)
	defer disconnectCancel()
	err = s.waitForSshToDisconnect(disconnectCtx)
	if err != nil {
		tc.FailFromError(fmt.Errorf("failed to detect SSH disconnection after Trident A/B update: %w", err))
	}

	logrus.Info("SSH client disconnected, host is rebooting. Will attempt to reconnect...")

	// Then, try to reconnect via SSH and check that Trident is running.
	conn_ctx, connCancel := context.WithTimeout(tc.Context(), time.Minute*5)
	defer connCancel()
	err = s.populateSshClient(conn_ctx)
	if err != nil {
		tc.FailFromError(err)
		return nil
	}

	logrus.Info("Reacquired SSH connection to host after reboot.")

	// Give it some extra time to ensure Trident is up after reboot.
	err = trident.CheckTridentService(s.sshClient, s.runtime, time.Minute*2, true)
	if err != nil {
		tc.FailFromError(err)
	}

	return nil
}

func runTridentUpdate(tc storm.TestCase, runtime trident.RuntimeType, client *ssh.Client, args string) error {
	for i := 1; ; i++ {
		logrus.Infof("Invoking Trident attempt #%d with args: %s", i, args)

		out, err := trident.InvokeTrident(runtime, client, nil, args)
		if err != nil {
			if err, ok := err.(*ssh.ExitMissingError); ok && strings.Contains(out.Stderr, trident.REBOOTING_LOG_MESSAGE) {
				// The connection closed without an exit code, and the output contains REBOOTING_LOG_MESSAGE.
				// This indicates that the host has rebooted.
				logrus.Infof("Host rebooted successfully")
				break
			} else {
				// Some unknown error occurred.
				logrus.Errorf("Failed to invoke Trident: %s; %s", err, out.Report())
				return fmt.Errorf("failed to invoke Trident: %w", err)
			}
		}

		if out.Status == 0 && strings.Contains(out.Stderr, "Staging of A/B update succeeded") {
			logrus.Infof("Staging of A/B update succeeded")
			break
		}

		if out.Status == 2 && strings.Contains(out.Stderr, "Failed to run post-configure script 'fail-on-the-first-run'") {
			logrus.Infof("Detected intentional failure. Re-running...")
			continue
		}

		logrus.Errorf("Trident update failed %s", out.Report())

		tc.Fail(fmt.Sprintf("Trident update failed with status %d", out.Status))
	}

	return nil
}

func checkUrlIsAccessible(url string) error {
	resp, err := http.Head(url)
	if err != nil {
		return fmt.Errorf("failed to check new image URL: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("new image URL is not accessible: %s, got HTTP code: %d", url, resp.StatusCode)
	}

	return nil
}
