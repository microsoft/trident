package scenario

import (
	"context"
	"errors"
	"fmt"
	"os"
	"time"
	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/phonehome"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
	log "github.com/sirupsen/logrus"
)

func (s *TridentE2EScenario) installOs(tc storm.TestCase) error {
	// Bump the version for this installation
	s.version += 1

	// Get netlaunch connection config
	connConfig := s.testHost.NetlaunchConnectionConfig()

	// Prepare host config
	hostConfigFile, err := s.config.ToYaml()
	if err != nil {
		return err
	}

	// Prepare temporary host config file to be used by netlaunch
	tempHostConfigFilePath, err := prepareHostConfig(hostConfigFile)
	if err != nil {
		return err
	}
	defer os.Remove(tempHostConfigFilePath)

	traceStreamFile := s.args.TracestreamFile
	if traceStreamFile == "" {
		traceStreamFile = "trident-clean-install-metrics.jsonl"
	}

	hc, err := os.ReadFile(tempHostConfigFilePath)
	if err != nil {
		return fmt.Errorf("failed to read prepared host config file: %w", err)
	}

	log.Infof("Using host config:\n%s", string(hc))

	config := netlaunch.NetLaunchConfig{
		NetCommonConfig: netlaunch.NetCommonConfig{
			ListenPort:           defaultNetlaunchListenPort,
			LogstreamFile:        s.args.LogstreamFile,
			TracestreamFile:      traceStreamFile,
			ServeDirectory:       s.args.TestImageDir,
			MaxPhonehomeFailures: s.configParams.MaxExpectedFailures,
		},
		Netlaunch:           connConfig,
		IsoPath:             s.args.IsoPath,
		WaitForProvisioning: true,
		HostConfigFile:      tempHostConfigFilePath,
		CertificateFile:     s.args.CertFile,
		EnableSecureBoot:    true,
	}

	timeoutCtx, cancel := context.WithTimeout(tc.Context(), time.Duration(s.args.VmWaitForLoginTimeout)*time.Second)
	defer cancel()

	// Start VM serial monitor (only runs if hardware is VM)
	monWaitChan, monErr := s.spawnVMSerialMonitor(timeoutCtx, tc.ArtifactBroker().StreamArtifactData("install/serial.log"))
	if monErr != nil {
		return fmt.Errorf("failed to start VM serial monitor: %w", monErr)
	}

	nlErr := netlaunch.RunNetlaunch(timeoutCtx, &config)
	if nlErr != nil {
		// If this is a phonehome error, log the details and fail the test case
		// immediately.
		var phonehomeErr *phonehome.PhoneHomeFailureError
		if errors.As(nlErr, &phonehomeErr) {
			log.Errorf("Phonehome error details: %s", phonehomeErr.Message)
			tc.FailFromError(nlErr)
		}

		// If this is a timeout error, log and fail the test case.
		if errors.Is(nlErr, context.DeadlineExceeded) {
			log.Errorln("Netlaunch operation timed out")
			tc.FailFromError(nlErr)
		}

		// Otherwise just return the error
		return nlErr
	}

	// If we got here netlaunch completed successfully, give some time for the
	// serial monitor to get to the login prompt.
	select {
	case <-time.After(time.Minute):
		log.Infof("Waited 1 minute for serial monitor to reach login prompt, cancelling monitor.")
		cancel()
	case <-monWaitChan:
		// Monitor exited on its own
	}

	return nil
}

func prepareHostConfig(hostConfigYaml []byte) (string, error) {
	tempHostConfigFile, err := os.CreateTemp("", "hc-tmp-")
	if err != nil {
		return "", fmt.Errorf("failed to create temporary host config file: %w", err)
	}

	defer func() {
		// Clean up the temp file on error
		if err != nil {
			os.Remove(tempHostConfigFile.Name())
		}

		// Close the file descriptor
		tempHostConfigFile.Close()
	}()

	_, err = tempHostConfigFile.Write(hostConfigYaml)
	if err != nil {
		return "", fmt.Errorf("failed to write to temporary host config file: %w", err)
	}

	err = tempHostConfigFile.Sync()
	if err != nil {
		return "", fmt.Errorf("failed to sync temporary host config file: %w", err)
	}

	return tempHostConfigFile.Name(), nil
}

func (s *TridentE2EScenario) checkTridentViaSshAfterInstall(tc storm.TestCase) error {
	// Short timeout since we're expecting the host to already be up.
	conn_ctx, cancel := context.WithTimeout(tc.Context(), time.Minute)
	defer cancel()
	err := s.populateSshClient(conn_ctx)
	if err != nil {
		tc.FailFromError(err)
		return nil
	}

	err = trident.CheckTridentService(s.sshClient, s.runtime, time.Minute*2, true)
	if err != nil {
		tc.FailFromError(err)
	}

	return nil
}
