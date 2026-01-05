package scenario

import (
	"context"
	"errors"
	"fmt"
	"os"
	"time"
	"tridenttools/pkg/netlaunch"
	"tridenttools/pkg/phonehome"

	"github.com/microsoft/storm"
	log "github.com/sirupsen/logrus"
)

func (s *TridentE2EScenario) installOs(tc storm.TestCase) error {
	connConfig := s.testHost.NetlaunchConnectionConfig()

	// Prepare host config
	hostConfigFile, err := s.renderHostConfiguration()
	if err != nil {
		return err
	}

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
		Netlaunch:            connConfig,
		IsoPath:              s.args.IsoPath,
		ListenPort:           defaultNetlaunchListenPort,
		HostConfigFile:       tempHostConfigFilePath,
		LogstreamFile:        s.args.LogstreamFile,
		TracestreamFile:      traceStreamFile,
		ServeDirectory:       s.args.TestImageDir,
		CertificateFile:      s.args.CertFile,
		EnableSecureBoot:     true,
		WaitForProvisioning:  true,
		MaxPhonehomeFailures: s.configParams.MaxExpectedFailures,
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

func prepareHostConfig(hostConfigYaml string) (string, error) {
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

	_, err = tempHostConfigFile.WriteString(hostConfigYaml)
	if err != nil {
		return "", fmt.Errorf("failed to write to temporary host config file: %w", err)
	}

	err = tempHostConfigFile.Sync()
	if err != nil {
		return "", fmt.Errorf("failed to sync temporary host config file: %w", err)
	}

	return tempHostConfigFile.Name(), nil
}
