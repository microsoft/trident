package scenario

import (
	"context"
	"time"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
)

func (s *TridentE2EScenario) checkTridentViaSsh(tc storm.TestCase) error {
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
