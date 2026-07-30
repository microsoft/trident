package scenario

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
	"gopkg.in/yaml.v3"

	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/netlisten"
	"tridenttools/storm/utils/ssh/sftp"
	"tridenttools/storm/utils/trident"
)

const (
	tridentDatastorePath = "/var/lib/trident/datastore.sqlite"
	tridentFullLogPath   = "/var/log/trident-full.log"
)

// addManualRollbackTests registers the manual-rollback test cases. After the
// A/B updates leave the host on one volume, a manually-triggered `trident
// rollback` returns it to the previously-committed volume; the state is then
// validated. This ports the legacy VM-only `storm-trident helper
// manual-rollback` step plus its follow-up pytest validation, and self-selects
// with the HasABUpdate() && IsVM() gate at the call site.
func (s *TridentE2EScenario) addManualRollbackTests(r storm.TestRegistrar) {
	r.RegisterTestCase("manual-rollback", s.manualRollback)
	r.RegisterTestCase("validate-manual-rollback", s.validateHostState)
}

// manualRollback stages and finalizes a `trident rollback`, which reboots the
// host back into the previously-committed OS, then confirms the rollback
// committed successfully and flips the expected active volume so the follow-up
// validation checks the rolled-back volume.
func (s *TridentE2EScenario) manualRollback(tc storm.TestCase) error {
	if err := s.populateSshClient(tc.Context()); err != nil {
		return fmt.Errorf("failed to connect before manual rollback: %w", err)
	}

	// Capture the rollback chain and the pre-rollback datastore as artifacts for
	// debugging, mirroring the legacy manual-rollback helper.
	if err := s.publishRollbackChain(tc); err != nil {
		return err
	}
	if err := s.publishArtifactFile(tc, tridentDatastorePath, "pre-rollback-datastore.sqlite"); err != nil {
		return err
	}

	// Serve phonehome + capture the serial log across the rollback reboot.
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
	monWaitChan, monErr := s.spawnVMSerialMonitor(monitorCtx, tc.ArtifactBroker().StreamArtifactData(tc.Name()+"/serial.log"))
	if monErr != nil {
		return fmt.Errorf("failed to start VM serial monitor: %w", monErr)
	}
	defer func() {
		select {
		case <-time.After(time.Minute):
			logrus.Infof("Waited 1 minute for serial monitor to reach login prompt, cancelling monitor.")
			cancel()
		case <-monWaitChan:
		}
	}()

	// Stage the rollback.
	logrus.Infof("Staging manual rollback...")
	stageOut, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "rollback -v trace --allowed-operations stage")
	if err != nil {
		return fmt.Errorf("failed to stage manual rollback: %w", err)
	}
	if err := stageOut.Check(); err != nil {
		return fmt.Errorf("manual rollback staging failed: %s", stageOut.Report())
	}
	_ = s.publishArtifactFile(tc, tridentFullLogPath, "rollback-staging.log")

	// Finalize the rollback: this reboots the host into the previous OS.
	logrus.Infof("Finalizing manual rollback (host will reboot)...")
	finalizeOut, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "rollback -v trace --allowed-operations finalize")
	if err != nil {
		// The connection dropping without an exit status alongside the reboot
		// message is the expected signal that the host has rebooted.
		if _, ok := err.(*ssh.ExitMissingError); ok && strings.Contains(finalizeOut.Stderr, trident.REBOOTING_LOG_MESSAGE) {
			logrus.Infof("Host rebooted successfully")
		} else {
			return fmt.Errorf("failed to finalize manual rollback: %s", finalizeOut.Report())
		}
	}

	// Wait for the reboot, reconnect, and confirm the rollback committed. A
	// manual rollback reboots exactly once (there are no health checks to fail),
	// so the standard single reconnect + successful-commit check applies.
	logrus.Info("Waiting for SSH client to disconnect after manual rollback...")
	disconnectCtx, cancel := context.WithTimeout(tc.Context(), time.Minute*2)
	defer cancel()
	if err := s.waitForSshToDisconnect(disconnectCtx); err != nil {
		tc.FailFromError(fmt.Errorf("failed to detect SSH disconnection after manual rollback: %w", err))
	}

	connCtx, cancel := context.WithTimeout(tc.Context(), time.Minute*5)
	defer cancel()
	if err := s.populateSshClient(connCtx); err != nil {
		tc.FailFromError(err)
		return nil
	}
	logrus.Info("Reacquired SSH connection to host after manual rollback reboot.")

	if err := trident.CheckTridentService(s.sshClient, s.runtime, time.Minute*2, true); err != nil {
		tc.FailFromError(err)
	}

	_ = s.publishArtifactFile(tc, tridentFullLogPath, "rollback-commit.log")

	// The rollback returned the host to the previously-committed volume, so the
	// expected active volume flips for the follow-up validation.
	s.expectedActiveVolume = s.expectedActiveVolume.Other()

	return nil
}

// publishRollbackChain fetches `trident get rollback-chain` and publishes it as
// a debugging artifact, failing if the chain cannot be read.
func (s *TridentE2EScenario) publishRollbackChain(tc storm.TestCase) error {
	out, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "get rollback-chain")
	if err != nil {
		return fmt.Errorf("failed to get rollback chain: %w", err)
	}
	if err := out.Check(); err != nil {
		return fmt.Errorf("failed to get rollback chain: %s", out.Report())
	}

	// Parse it only to log how many rollbacks are available; the raw output is
	// what we publish.
	var chain []map[string]interface{}
	if err := yaml.Unmarshal([]byte(strings.TrimSpace(out.Stdout)), &chain); err != nil {
		return fmt.Errorf("failed to parse rollback chain: %w", err)
	}
	logrus.Infof("Available rollbacks: %d", len(chain))

	tc.ArtifactBroker().PublishArtifactData("rollback-chain.yaml", []byte(out.Stdout))
	return nil
}

// publishArtifactFile downloads a remote file and publishes it as a named
// artifact.
func (s *TridentE2EScenario) publishArtifactFile(tc storm.TestCase, remotePath, artifactName string) error {
	localPath, err := sftp.DownloadRemoteFile(s.sshClient, remotePath, "")
	if err != nil {
		return fmt.Errorf("failed to download %s: %w", remotePath, err)
	}
	tc.ArtifactBroker().PublishLogFile(artifactName, localPath)
	return nil
}
