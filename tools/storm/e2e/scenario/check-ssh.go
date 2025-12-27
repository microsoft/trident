package scenario

import (
	"github.com/microsoft/storm"
)

func (s *TridentE2EScenario) checkTridentViaSsh(tc storm.TestCase) error {
	// err := check.CheckTridentService(
	// 	s.getSshCliSettings(),
	// 	s.args.EnvCliSettings,
	// 	expectSuccessfulCommit,
	// 	s.args.TimeoutDuration(),
	// 	tc,
	// )
	// if err != nil {
	// 	logrus.Errorf("Trident service check via SSH failed: %s", err)
	// 	tc.FailFromError(err)
	// }
	return nil
}
