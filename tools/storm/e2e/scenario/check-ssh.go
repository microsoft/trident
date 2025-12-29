package scenario

import (
	"context"
	"time"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/trident"

	"github.com/microsoft/storm"
)

func (s *TridentE2EScenario) checkTridentViaSsh(tc storm.TestCase) error {
	conn_ctx, cancel := context.WithTimeout(tc.Context(), time.Minute)
	defer cancel()
	client, err := sshutils.CreateSshClientWithRedial(conn_ctx, time.Second, s.sshClientConfig())
	if err != nil {
		tc.FailFromError(err)
		return nil
	}

	err = trident.CheckTridentService(client, s.runtime, time.Minute*2, true)
	if err != nil {
		tc.FailFromError(err)
	}

	return nil
}
