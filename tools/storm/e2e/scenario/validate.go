package scenario

import (
	"context"
	"time"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"

	"tridenttools/storm/e2e/validate"
	"tridenttools/storm/utils/trident"
)

// hasRollbackIntent reports whether this scenario is expected to trigger a
// health-check rollback, signalled by a top-level `health` section in the Host
// Configuration. Such scenarios expect the install to fail and roll back rather
// than commit successfully.
func (s *TridentE2EScenario) hasRollbackIntent() bool {
	return s.config.Exists("health")
}

// validateHostState is the storm test case that validates the installed host's
// state against the Host Configuration and Host Status. It is registered after
// clean install and after each A/B update. It ports the pytest E2E validation
// suite (tests/e2e_tests/*.py).
//
// All applicable sub-checks run and accumulate their failures (interim
// soft-assert approach) so a single case reports every mismatch it finds; the
// case fails once at the end if any sub-check failed.
func (s *TridentE2EScenario) validateHostState(tc storm.TestCase) error {
	connCtx, cancel := context.WithTimeout(tc.Context(), time.Minute)
	defer cancel()
	if err := s.populateSshClient(connCtx); err != nil {
		// The host is expected to be up by now, so a connection failure is an
		// infrastructure error rather than a product failure.
		return err
	}

	hs, err := trident.GetHostStatus(s.runtime, s.sshClient)
	if err != nil {
		return err
	}

	var sa validate.SoftAsserter

	if validate.HasRollbackIntent(hs) {
		// Health-check rollback scenarios (e.g. health-checks-install) replace
		// base validation with rollback validation and expect the host to have
		// rolled back rather than reached the provisioned state.
		validate.ValidateRollback(&sa, s.sshClient, hs,
			trident.ServicingStateNotProvisioned, s.expectedActiveVolume)
	} else {
		// `base` validation always applies.
		validate.ValidateBase(&sa, s.sshClient, hs, trident.ServicingStateProvisioned, s.expectedActiveVolume)

		// `extensions` validation self-selects when the Host Config declares
		// sysexts/confexts.
		if validate.HasExtensions(hs) {
			validate.ValidateExtensions(&sa, s.sshClient, hs)
		}

		// `verity` validation self-selects when the Host Config declares a
		// verity device.
		if validate.HasVerity(hs) {
			validate.ValidateVerity(&sa, s.sshClient, hs, s.expectedActiveVolume)
		}

		// `encryption` validation self-selects when the Host Config declares
		// encryption volumes.
		if validate.HasEncryption(hs) {
			validate.ValidateEncryption(&sa, s.sshClient, hs, s.configParams.IsUki, s.expectedActiveVolume)
		}
	}

	if err := sa.Err(); err != nil {
		tc.FailFromError(err)
	}

	// Always log the full ordered PASS/FAIL breakdown so a single validate case
	// surfaces exactly which sub-checks ran, even when it passes.
	logrus.Infof("Host state validation summary:\n%s", sa.Summary())

	return nil
}

// defaultCleanInstallMetricsFile is the trace-stream file netlisten writes for
// the clean install when no explicit tracestream file is configured. Kept in
// sync with installOs.
const defaultCleanInstallMetricsFile = "trident-clean-install-metrics.jsonl"

// cleanInstallTraceFile returns the local path of the trace-stream file netlisten
// captured for the clean install.
func (s *TridentE2EScenario) cleanInstallTraceFile() string {
	if s.args.TracestreamFile != "" {
		return s.args.TracestreamFile
	}
	return defaultCleanInstallMetricsFile
}

// validateHostDiagnostics ports the host-only check-selinux and check-tracing
// steps that legacy ran after clean install: it confirms no SELinux denials
// were logged (surfaced via audit2allow), that Trident's commit tracing metric
// reached journald, and that the servicing feature-usage metric was captured in
// the install trace-stream file. It self-skips on the container runtime, where
// these host-side concerns do not apply.
func (s *TridentE2EScenario) validateHostDiagnostics(tc storm.TestCase) error {
	if s.runtime != trident.RuntimeTypeHost {
		tc.Skip("Host diagnostics (SELinux + tracing) only apply to the host runtime")
	}

	connCtx, cancel := context.WithTimeout(tc.Context(), time.Minute)
	defer cancel()
	if err := s.populateSshClient(connCtx); err != nil {
		return err
	}

	var sa validate.SoftAsserter
	validate.ValidateSelinuxDenials(&sa, s.sshClient)
	validate.ValidateJournaldTracing(&sa, s.sshClient)
	validate.ValidateTraceFileMetric(&sa, s.cleanInstallTraceFile())

	if err := sa.Err(); err != nil {
		tc.FailFromError(err)
	}
	logrus.Infof("Host diagnostics validation summary:\n%s", sa.Summary())

	return nil
}

// Unlike validateHostState (which self-selects rollback validation only for
// scenarios whose Host Config declares a top-level `health` section), this case
// always asserts the rollback outcome: the failed update rolled back onto the
// current volume, so the host stays provisioned with the active volume
// unchanged. It ports rollback_test.py for the auto-rollback (provisioned)
// case.
func (s *TridentE2EScenario) validateAutoRollback(tc storm.TestCase) error {
	connCtx, cancel := context.WithTimeout(tc.Context(), time.Minute)
	defer cancel()
	if err := s.populateSshClient(connCtx); err != nil {
		return err
	}

	hs, err := trident.GetHostStatus(s.runtime, s.sshClient)
	if err != nil {
		return err
	}

	var sa validate.SoftAsserter
	validate.ValidateRollback(&sa, s.sshClient, hs,
		trident.ServicingStateProvisioned, s.expectedActiveVolume)

	if err := sa.Err(); err != nil {
		tc.FailFromError(err)
	}

	logrus.Infof("Auto-rollback validation summary:\n%s", sa.Summary())

	return nil
}
