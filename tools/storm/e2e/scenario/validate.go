package scenario

import (
	"context"
	"time"

	"github.com/microsoft/storm"

	"tridenttools/storm/e2e/validate"
	"tridenttools/storm/utils/trident"
)

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

	// `base` validation always applies.
	validate.ValidateBase(&sa, s.sshClient, hs, trident.ServicingStateProvisioned, s.expectedActiveVolume)

	// Future markers (encryption, verity, extensions) will self-select here
	// based on the Host Configuration.

	if err := sa.Err(); err != nil {
		tc.FailFromError(err)
	}

	return nil
}
