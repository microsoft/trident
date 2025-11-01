package e2e

import (
	"fmt"

	"github.com/microsoft/storm"

	"github.com/sirupsen/logrus"
)

type TridentE2EScenario struct {
	storm.BaseScenario
	name       string
	tags       []string
	stagePaths []string
	config     map[string]interface{}
	args       TridentE2EScenarioArgs
}

type TridentE2EScenarioArgs struct {
	StagePath TridentE2EStagePath `arg:"" help:"Stage path to run: 'e2e/<pipeline>/<machine>/<runtime>'"`
}

func CreateTridentScenario(name string) TridentE2EScenario {
	return TridentE2EScenario{
		name:       name,
		tags:       make([]string, 0),
		stagePaths: make([]string, 0),
		config:     make(map[string]interface{}),
		args: TridentE2EScenarioArgs{
			StagePath: NewTridentE2EStagePath(TridetPipelineLocal, TridentMachineVm, TridentRuntimeHost),
		},
	}
}

func (s *TridentE2EScenario) AddStagePath(path string) {
	s.stagePaths = append(s.stagePaths, path)
}

func (s *TridentE2EScenario) Name() string {
	return s.name
}

func (s *TridentE2EScenario) Args() any {
	return &s.args
}

func (s *TridentE2EScenario) Tags() []string {
	return s.tags
}

func (s *TridentE2EScenario) StagePaths() []string {
	return s.stagePaths
}

func (s *TridentE2EScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("run", s.Run)
	return nil
}

func (s TridentE2EScenario) Run(tc storm.TestCase) error {
	logrus.Infof("Hello from '%s'!", s.Name())
	logrus.Infof("Running stage '%s'", s.args.StagePath)

	fmt.Println(s.config)

	// TODO: Implement the actual scenario logic here

	return nil
}
