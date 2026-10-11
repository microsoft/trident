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

	signingCert, err := s.signingCertFile()
	if err != nil {
		return err
	}

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
		CertificateFile:     signingCert,
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
		// If this is a phonehome error, decide whether it is expected.
		var phonehomeErr *phonehome.PhoneHomeFailureError
		if errors.As(nlErr, &phonehomeErr) {
			if s.hasRollbackIntent() {
				// Rollback-intent scenarios (e.g. health-checks-install)
				// deliberately fail their health checks and roll back, so
				// Trident phones home a failure. This is the expected outcome:
				// the target OS booted (health checks ran) and is reachable, so
				// continue to post-install validation rather than failing.
				log.Infof("Expected phonehome failure for rollback-intent scenario: %s", phonehomeErr.Message)
			} else {
				log.Errorf("Phonehome error details: %s", phonehomeErr.Message)
				tc.FailFromError(nlErr)
			}
		} else if errors.Is(nlErr, context.DeadlineExceeded) {
			// If this is a timeout error, log and fail the test case.
			log.Errorln("Netlaunch operation timed out")
			tc.FailFromError(nlErr)
		} else {
			// Otherwise just return the error
			return nlErr
		}
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

	// Rollback-intent scenarios (e.g. health-checks-install) deliberately fail
	// their health checks during install and roll back, so the Trident service
	// is expected to exit with a non-zero status rather than commit
	// successfully.
	expectSuccessfulCommit := !s.hasRollbackIntent()

	err = trident.CheckTridentService(s.sshClient, s.runtime, time.Minute*2, expectSuccessfulCommit)
	if err != nil {
		tc.FailFromError(err)
	}

	return nil
}

// signingCertFile returns the image signing certificate to enroll into the VM's
// EFI variables, or "" when there is none.
//
// UKI/usr-verity images boot their kernel directly through firmware Secure
// Boot, so they need the certificate enrolled; it ships alongside the usrverity
// test image. Enrolling it is harmless for grub-based images, so the caller
// passes the path unconditionally and a grub-based configuration whose
// artifacts do not include one simply proceeds without it - rather than the
// caller having to test for the file first.
//
// For a UKI configuration the certificate is not optional: netlaunch only
// enrolls it when the path is non-empty, so silently dropping it boots a VM
// that cannot verify its own kernel, which surfaces much later as an
// unexplained boot timeout.
func (s *TridentE2EScenario) signingCertFile() (string, error) {
	if s.args.CertFile == "" {
		if s.configParams.IsUki {
			return "", fmt.Errorf("--signing-cert is required for UKI configurations, which boot through firmware Secure Boot")
		}
		return "", nil
	}
	// A directory satisfies both os.Stat here and netlaunch's os.Open
	// preflight, so without the regular-file check an invalid path is accepted
	// and only fails later, during VM firmware-variable setup, as an opaque
	// boot failure.
	if err := readableRegularFile(s.args.CertFile); err != nil {
		if s.configParams.IsUki {
			return "", fmt.Errorf("image signing certificate %q is required for UKI configurations but is unusable: %w",
				s.args.CertFile, err)
		}
		log.Infof("No usable image signing certificate at %q (%v); continuing without one.", s.args.CertFile, err)
		return "", nil
	}
	return s.args.CertFile, nil
}

// readableRegularFile reports whether path is a regular file the process can
// actually read, rather than merely something that exists.
func readableRegularFile(path string) error {
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() {
		return fmt.Errorf("not a regular file (mode %s)", info.Mode())
	}
	f, err := os.Open(path)
	if err != nil {
		return err
	}
	return f.Close()
}
