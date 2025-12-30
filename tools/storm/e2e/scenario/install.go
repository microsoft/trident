package scenario

import (
	"fmt"
	"os"
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
		NetCommonConfig: netlaunch.NetCommonConfig{
			LogstreamFile:        s.args.LogstreamFile,
			TracestreamFile:      traceStreamFile,
			ServeDirectory:       s.args.TestImageDir,
			WaitForProvisioning:  true,
			MaxPhonehomeFailures: s.configParams.MaxExpectedFailures,
		},
		Netlaunch:        connConfig,
		IsoPath:          s.args.IsoPath,
		ListenPort:       defaultNetlaunchListenPort,
		HostConfigFile:   tempHostConfigFilePath,
		CertificateFile:  s.args.CertFile,
		EnableSecureBoot: true,
	}

	nlErr := netlaunch.RunNetlaunch(tc.Context(), &config)
	if nlErr != nil {
		// If this is a phonehome error, log the details and fail the test case
		// immediately.
		if phonehomeErr, ok := nlErr.(*phonehome.PhoneHomeFailureError); ok {
			log.Errorf("Phonehome error details: %s", phonehomeErr.Message)
			tc.FailFromError(nlErr)
		}

		// Otherwise just return the error
		return nlErr
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
