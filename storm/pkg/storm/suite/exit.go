package suite

import (
	"fmt"
	"os"
	"storm/internal/devops"
)

// Exit the program and report the exit status
func (s *StormSuite) reportExitStatus(err error) {
	if err == nil {
		s.Log.Infof("Suite '%s' run completed", s.name)
		os.Exit(0)
	}

	if s.azureDevops {
		devops.LogError(fmt.Sprintf("Suite '%s' run failed: %s", s.name, err))
	}

	s.Log.WithError(err).Fatalf("Suite '%s' failed", s.name)
}
