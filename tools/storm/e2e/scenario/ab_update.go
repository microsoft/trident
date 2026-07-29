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
	"tridenttools/storm/e2e/validate"
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
		return s.abUpdateOs(tc, abUpdateOptions{})
	})
}

// splitTestsSkippedForCurrentRing reports whether split A/B update testing is
// skipped on the current ring. The lowest ring for which we run split testing
// is 'prerelease'.
func (s *TridentE2EScenario) splitTestsSkippedForCurrentRing() bool {
	return s.args.TestRing < testrings.TestRingPre
}

// skipIfSplitTestsDisabled marks tc as skipped (and, via storm, stops it) when
// split A/B update testing does not run on the current ring. Shared by the
// split A/B update test cases and their validation so they skip together.
func (s *TridentE2EScenario) skipIfSplitTestsDisabled(tc storm.TestCase) {
	if s.splitTestsSkippedForCurrentRing() {
		tc.Skip(fmt.Sprintf("Skipping split AB update test on ring '%s'", s.args.TestRing.ToString()))
	}
}

// addSplitABUpdateTests adds the split A/B update test cases to the provided test registrar
func (s *TridentE2EScenario) addSplitABUpdateTests(r storm.TestRegistrar, prefix string) {
	filterSplitTestForCurrentRing := func(s *TridentE2EScenario, tc storm.TestCase, testFn func(storm.TestCase) error) error {
		s.skipIfSplitTestsDisabled(tc)
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
	r.RegisterTestCase(prefix+"-ab-update", func(tc storm.TestCase) error {
		return filterSplitTestForCurrentRing(s, tc, func(tc storm.TestCase) error {
			return s.abUpdateOs(tc, abUpdateOptions{split: true})
		})
	})
}

// Health check names injected to force an A/B-update rollback. They mirror the
// checks the legacy pipeline adds via `storm-trident helper ab-update
// --forced-rollback` and produce the failure-log messages asserted by rollback
// validation (see validate.ValidateRollback).
const (
	rollbackScriptCheckName  = "invoke-rollback-from-script"
	rollbackSystemdCheckName = "check-non-existent-service-to-invoke-rollback"
)

// addAutoRollbackTests registers the auto-rollback test cases: they force an
// A/B update to fail its health checks so Trident rolls back to the current
// volume, then validate the rolled-back state. This ports the legacy
// e2e-test-abupdate-scenario.yml `--forced-rollback` flow and self-selects with
// the surrounding HasABUpdate() gate.
func (s *TridentE2EScenario) addAutoRollbackTests(r storm.TestRegistrar) {
	r.RegisterTestCase("auto-rollback-sync-hc", s.syncHostConfig)
	r.RegisterTestCase("auto-rollback-inject-hc", s.injectRollbackHealthChecks)
	r.RegisterTestCase("auto-rollback-upload-hc", s.uploadNewConfig)
	r.RegisterTestCase("auto-rollback-update", func(tc storm.TestCase) error {
		return s.abUpdateOs(tc, abUpdateOptions{expectRollback: true})
	})
	r.RegisterTestCase("validate-auto-rollback", s.validateAutoRollback)
}

// injectRollbackHealthChecks mutates the synced Host Config so the next A/B
// update fails its health checks and rolls back. It blocks Trident
// self-upgrade, drops the storage section (not needed for an update, matching
// the regular update path), and appends a script check that always fails plus
// a systemd check on non-existent services, both gated to the ab-update phase.
// The image URL is left untouched so the rolled-back update re-stages the same
// image (the legacy scenario runs with incrementUpdateVersion=false).
func (s *TridentE2EScenario) injectRollbackHealthChecks(tc storm.TestCase) error {
	s.config.Set(false, "internalParams", "selfUpgradeTrident")
	s.config.Delete("storage")

	scriptCheck := map[string]interface{}{
		"name":    rollbackScriptCheckName,
		"content": "exit 1",
		"runOn":   []interface{}{"ab-update"},
	}
	if err := s.config.ArrayAppend(scriptCheck, "health", "checks"); err != nil {
		return fmt.Errorf("failed to add rollback script health check: %w", err)
	}

	systemdCheck := map[string]interface{}{
		"name":            rollbackSystemdCheckName,
		"runOn":           []interface{}{"ab-update"},
		"systemdServices": []interface{}{"non-existent-service1", "non-existent-service2"},
		"timeoutSeconds":  30,
	}
	if err := s.config.ArrayAppend(systemdCheck, "health", "checks"); err != nil {
		return fmt.Errorf("failed to add rollback systemd health check: %w", err)
	}

	return nil
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

// abUpdateOptions controls a single A/B update run performed by abUpdateOs.
type abUpdateOptions struct {
	// split stages and finalizes the update as two independent operations,
	// validating the staged state in between.
	split bool
	// expectRollback runs an update that is expected to fail its health checks
	// and auto-roll-back: the commit is expected to fail and the host returns
	// to its current active volume rather than flipping to the other one.
	expectRollback bool
}

func (s *TridentE2EScenario) abUpdateOs(tc storm.TestCase, opts abUpdateOptions) error {
	updateCmd := "update"
	if s.runtime == trident.RuntimeTypeHost {
		// For host, use grpc-client for update
		updateCmd = "grpc-client update"
	}

	args := fmt.Sprintf(
		"%s -v trace %s",
		updateCmd,
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

	if !opts.split {
		// regular case
		logrus.Infof("Running Trident A/B update...")
		err = runTridentUpdate(tc, s.runtime, s.sshClient, args, false)
		if err != nil {
			return fmt.Errorf("failed to run Trident A/B update: %w", err)
		}
	} else {
		// split stage and finalize
		logrus.Infof("Running split Trident A/B update (stage)...")
		err = runTridentUpdate(tc, s.runtime, s.sshClient, args+" --allowed-operations stage", true)
		if err != nil {
			return fmt.Errorf("failed to run Trident A/B update: %w", err)
		}

		// Between stage and finalize, validate the staged state. The active
		// volume must not have changed yet (it flips only after finalize+reboot).
		stagedHs, err := trident.GetHostStatus(s.runtime, s.sshClient)
		if err != nil {
			return fmt.Errorf("failed to get Host Status after staging A/B update: %w", err)
		}
		var sa validate.SoftAsserter
		validate.ValidateAbUpdateStaged(&sa, stagedHs, s.expectedActiveVolume)
		if stagedErr := sa.Err(); stagedErr != nil {
			tc.FailFromError(stagedErr)
		}

		logrus.Infof("Running split Trident A/B update (finalize)...")
		err = runTridentUpdate(tc, s.runtime, s.sshClient, args+" --allowed-operations finalize", false)
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
	// A forced rollback is expected to leave the commit in a failed state (the
	// update's health checks failed and Trident rolled back), whereas a normal
	// A/B update must commit successfully.
	err = trident.CheckTridentService(s.sshClient, s.runtime, time.Minute*2, !opts.expectRollback)
	if err != nil {
		tc.FailFromError(err)
	}

	// A committed A/B update reboots into the other volume; a rollback returns
	// to the current one. Only flip the expected active volume when the update
	// was expected to commit.
	if !opts.expectRollback {
		s.expectedActiveVolume = s.expectedActiveVolume.Other()
	}

	return nil
}

func runTridentUpdate(tc storm.TestCase, runtime trident.RuntimeType, client *ssh.Client, args string, stagingOnly bool) error {
	for i := 1; ; i++ {
		logrus.Infof("Invoking Trident attempt #%d with args: %s", i, args)

		out, err := trident.InvokeTrident(runtime, client, nil, args)
		logrus.Tracef("Trident '%s' details: %s; %s", args, err, out.Report())

		// If this is not staging only, check and exit if reboot message found in output
		if !stagingOnly && strings.Contains(out.Stderr, trident.REBOOTING_LOG_MESSAGE) {
			logrus.Infof("Host rebooted successfully")
			break
		}
		// Errors that occur without the reboot message are a test failure
		if err != nil {
			return fmt.Errorf("failed to invoke Trident: %w", err)
		}
		// If only staging, check for staging success message in output
		if stagingOnly && out.Status == 0 && strings.Contains(out.Stderr, "Staging of A/B update succeeded") {
			logrus.Infof("Staging of A/B update succeeded")
			break
		}
		// Check for specific failure case representing a retry e2e scenario
		if out.Status == 2 && strings.Contains(out.Stderr, "Failed to run post-configure script 'fail-on-the-first-run'") {
			logrus.Infof("Detected intentional failure. Re-running...")
			continue
		}
		// Any case where we end up here is a failure, fail the test
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
