package suite

import (
	"slices"
	"storm/internal/cli"

	"fmt"
	"storm/pkg/storm/core"

	"github.com/alecthomas/kong"
	"github.com/sirupsen/logrus"
)

type StormSuite struct {
	name      string
	scenarios []core.Scenario
	ctx       *kong.Context
	Log       *logrus.Logger
	helpers   []core.Helper
}

func CreateSuite(name string) StormSuite {
	name = fmt.Sprintf("storm-%s", name)
	ctx, global := cli.ParseCommandLine(name)
	logger := logrus.New()
	logger.SetLevel(global.Verbosity)
	logger.SetFormatter(&logrus.TextFormatter{
		ForceColors: true,
	})

	logger.Infof("Creating suite '%s'", name)

	return StormSuite{
		name:      name,
		scenarios: make([]core.Scenario, 0),
		ctx:       ctx,
		Log:       logger,
	}
}

// Run the storm suite
func (s *StormSuite) Run() {
	if s.ctx == nil {
		s.Log.Fatalf("Suite '%s' not initialized", s.name)
	}

	s.Log.Infof("Running suite '%s' - %d scenarios, %d helpers collected.", s.name, len(s.scenarios), len(s.helpers))
	s.ctx.BindTo(s, (*core.SuiteContext)(nil))
	err := s.ctx.Run()
	s.ctx.FatalIfErrorf(err)
}

// Adds a scenario to the suite
func (s *StormSuite) AddScenario(new_scenario core.Scenario) {
	if slices.ContainsFunc(s.scenarios, func(scenario core.Scenario) bool {
		return scenario.Name() == new_scenario.Name()
	}) {
		s.Log.Fatalf("Scenario '%s' already exists", new_scenario.Name())
	}

	s.Log.Debugf("Registering scenario '%s'", new_scenario.Name())
	s.Log.Tracef("Tags: %v", new_scenario.Tags())
	s.Log.Tracef("Stage paths: %v", new_scenario.StagePaths())
	s.scenarios = append(s.scenarios, new_scenario)
}

// Adds a helper to the suite
func (s *StormSuite) AddHelper(helper core.Helper) {
	if slices.ContainsFunc(s.helpers, func(h core.Helper) bool {
		return h.Name() == helper.Name()
	}) {
		s.Log.Fatalf("Helper '%s' already exists", helper.Name())
	}
	s.Log.Debugf("Registering helper '%s'", helper.Name())
	s.helpers = append(s.helpers, helper)
}

// Returns the name of the suite
func (s *StormSuite) Name() string {
	return s.name
}

// Returns a list of all scenarios
func (s *StormSuite) Scenarios() []core.Scenario {
	return s.scenarios
}

// Returns a scenario by name, will exit with an error if the scenario is not
// found.
func (s *StormSuite) Scenario(name string) core.Scenario {
	for _, scenario := range s.scenarios {
		if scenario.Name() == name {
			return scenario
		}
	}

	s.Log.Fatalf("Scenario '%s' not found", name)
	return nil
}

// Returns a list of all helpers
func (s *StormSuite) Helpers() []core.Helper {
	return s.helpers
}

// Returns a helper by name, will exit with an error if the helper is not
// found.
func (s *StormSuite) Helper(name string) core.Helper {
	for _, helper := range s.helpers {
		if helper.Name() == name {
			return helper
		}
	}

	s.Log.Fatalf("Helper '%s' not found", name)
	return nil
}

func (s *StormSuite) Logger() *logrus.Logger {
	return s.Log
}
