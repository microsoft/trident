package scenario

import "github.com/microsoft/storm"

func (s *TridentE2EScenario) AddAbUpdateTests(r storm.TestRegistrar, prefix string) {
	r.RegisterTestCase(prefix+"-update-hc", s.updateHostConfig)
}

func (s *TridentE2EScenario) updateHostConfig(tc storm.TestCase) error {

}
