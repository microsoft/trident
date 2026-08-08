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
	"tridenttools/storm/utils/retry"
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
	r.RegisterTestCase("auto-rollback-update-hc", s.updateHostConfig)
	r.RegisterTestCase("auto-rollback-inject-hc", s.injectRollbackHealthChecks)
	r.RegisterTestCase("auto-rollback-upload-hc", s.uploadNewConfig)
	r.RegisterTestCase("auto-rollback-update", func(tc storm.TestCase) error {
		return s.abUpdateOs(tc, abUpdateOptions{expectRollback: true})
	})
	r.RegisterTestCase("validate-auto-rollback", s.validateAutoRollback)
}

// addSecondAbUpdateTests registers the second A/B update: the return update into
// OS A after the auto-rollback left the host on OS B. It mirrors legacy's
// "Stage and finalize A/B update into target OS A": a normal (committing) A/B
// update that reuses the auto-rollback's image version (see
// updateHostConfigReuseVersion) so the image UUID differs from the active
// volume. It also clears the failing health checks the auto-rollback injected,
// so this update commits instead of rolling back. Self-selects with the
// surrounding HasABUpdate() gate.
func (s *TridentE2EScenario) addSecondAbUpdateTests(r storm.TestRegistrar) {
	r.RegisterTestCase("ab-update-2-sync-hc", s.syncHostConfig)
	r.RegisterTestCase("ab-update-2-clear-hc", s.removeRollbackHealthChecks)
	r.RegisterTestCase("ab-update-2-update-hc", s.updateHostConfigReuseVersion)
	r.RegisterTestCase("ab-update-2-upload-new-hc", s.uploadNewConfig)
	r.RegisterTestCase("ab-update-2-ab-update", func(tc storm.TestCase) error {
		return s.abUpdateOs(tc, abUpdateOptions{})
	})
	r.RegisterTestCase("validate-ab-update-2", s.validateHostState)
}

// removeRollbackHealthChecks strips the failing health checks that the
// auto-rollback step injected, so the following A/B update commits instead of
// rolling back again. It is tolerant of the checks being absent (the synced
// config may already be clean). Mirrors the legacy ab-update helper removing
// those checks when it runs without --forced-rollback.
func (s *TridentE2EScenario) removeRollbackHealthChecks(tc storm.TestCase) error {
	checks := s.config.S("health", "checks")
	if checks == nil {
		return nil
	}

	kept := make([]interface{}, 0)
	for _, check := range checks.Children() {
		name, _ := check.S("name").Data().(string)
		if name == rollbackScriptCheckName || name == rollbackSystemdCheckName {
			continue
		}
		kept = append(kept, check.Data())
	}
	s.config.Set(kept, "health", "checks")
	return nil
}

// injectRollbackHealthChecks appends failing health checks to the (already
// image-bumped) Host Config so the next A/B update fails and rolls back: a
// script check that always fails plus a systemd check on non-existent services,
// both gated to the ab-update phase. It runs after updateHostConfig, which
// already points the config at a fresh image (so the update passes duplicate
// filesystem-UUID validation and actually reaches the health-check phase),
// drops storage, and blocks self-upgrade. Mirrors the failing checks the legacy
// ab-update helper adds for --forced-rollback.
func (s *TridentE2EScenario) injectRollbackHealthChecks(tc storm.TestCase) error {
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
	return s.updateHostConfigToVersion(tc, true)
}

// updateHostConfigReuseVersion points the Host Config at the current image
// version without bumping it. It is used for the second A/B update (the return
// into OS A after an auto-rollback), which must reuse the auto-rollback's image
// version: that version is odd (aliased to the base image, image.cosi) and so
// has a filesystem UUID distinct from the currently-active volume, whereas the
// next even version would alias image_v2.cosi and collide with it. This mirrors
// legacy's auto-rollback step running with incrementUpdateVersion=false so the
// subsequent A/B update into OS A reuses the same image version.
func (s *TridentE2EScenario) updateHostConfigReuseVersion(tc storm.TestCase) error {
	return s.updateHostConfigToVersion(tc, false)
}

func (s *TridentE2EScenario) updateHostConfigToVersion(tc storm.TestCase, bump bool) error {
	// Bump the image version by 1 unless the caller wants to reuse the current
	// version (see updateHostConfigReuseVersion).
	if bump {
		s.version += 1
	}

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

	// After a committed A/B update the host reboots and SSH drops; wait for that
	// before reconnecting.
	logrus.Info("Waiting for SSH client to disconnect after Trident A/B update...")
	disconnectCtx, cancel := context.WithTimeout(tc.Context(), time.Minute*2)
	defer cancel()
	err = s.waitForSshToDisconnect(disconnectCtx)
	if err != nil {
		// Both a normal A/B update and a forced rollback reboot the host (the
		// rollback reboots into the staged volume to run the failing health
		// checks), so a missing disconnect is a test failure either way.
		tc.FailFromError(fmt.Errorf("failed to detect SSH disconnection after Trident A/B update: %w", err))
	}

	logrus.Info("SSH client disconnected, host is rebooting. Will attempt to reconnect...")

	if opts.expectRollback {
		// A forced rollback reboots TWICE: into the staged volume to run the
		// failing health checks, then back to the current volume once the
		// rollback is applied. A single reconnect can land on the host mid-way
		// through the second reboot, so re-dial a fresh client and re-check
		// until the service settles in its failed-commit state.
		if err := s.waitForFailedCommitAfterRollback(tc); err != nil {
			tc.FailFromError(err)
		}
		// The rollback returned the host to its current active volume, so the
		// expected active volume is left unchanged.
		return nil
	}

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

	// The A/B update rebooted into the other volume; flip the expected active
	// volume so subsequent validation checks the correct one.
	s.expectedActiveVolume = s.expectedActiveVolume.Other()

	return nil
}

// waitForFailedCommitAfterRollback reconnects to the host after a forced
// rollback and waits until the Trident service reports its failed-commit state.
// It re-dials a fresh SSH client on every attempt so it tolerates the second
// reboot the rollback performs (into the staged volume for health checks, then
// back to the current volume). Mirrors the retry-with-fresh-client behaviour of
// the legacy `check-trident-service` helper.
func (s *TridentE2EScenario) waitForFailedCommitAfterRollback(tc storm.TestCase) error {
	const settleTimeout = time.Minute * 8
	overallCtx, cancel := context.WithTimeout(tc.Context(), settleTimeout)
	defer cancel()

	_, err := retry.Retry(settleTimeout, time.Second*10, func(attempt int) (*bool, error) {
		logrus.Infof("Checking for settled rollback state (attempt %d)", attempt)

		// Force a fresh dial: drop any stale client so populateSshClient
		// reconnects rather than reusing a connection killed by the reboot.
		if s.sshClient != nil {
			s.sshClient.Close()
			s.sshClient = nil
		}
		if err := s.populateSshClient(overallCtx); err != nil {
			return nil, fmt.Errorf("failed to reconnect after rollback: %w", err)
		}

		// expectSuccessfulCommit=false: the update's health checks failed and
		// Trident rolled back, so the service must report a failed commit.
		if err := trident.CheckTridentService(s.sshClient, s.runtime, time.Second*30, false); err != nil {
			return nil, err
		}
		return nil, nil
	})
	if err != nil {
		return fmt.Errorf("host did not settle into the expected failed-commit state after rollback: %w", err)
	}

	logrus.Info("Host settled into the expected failed-commit (rolled-back) state.")
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
