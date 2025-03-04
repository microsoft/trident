package list

import (
	"fmt"
	"storm/pkg/storm/core"
	"storm/pkg/storm/utils"
)

type ListScenariosCmd struct {
	Tags               []string `short:"t" long:"tags" description:"Filter scenarios by tags"`
	StagePaths         []string `short:"s" long:"stage" description:"Filter scenarios by stage paths"`
	RecusiveStagePaths bool     `short:"r" long:"recursive" description:"Filter scenarios by stage paths recursively"`
}

func (cmd *ListScenariosCmd) Run(suite core.SuiteContext) error {
	log := suite.Logger()
	log.Info("Listing scenarios")

	tagFilter := utils.NewStringFilterFromSlice(cmd.Tags)
	stagePathFilter := utils.NewPathFilterFromSlice(cmd.StagePaths, cmd.RecusiveStagePaths)

	collected := 0
	for _, scenario := range suite.Scenarios() {
		log.Tracef("Checking scenario '%s'", scenario.Name())

		if !tagFilter.MatchAny(scenario.Tags()) {
			log.Tracef("Skipping scenario '%s' because it does not match any tags", scenario.Name())
			continue
		}

		if !stagePathFilter.MatchAny(scenario.StagePaths()) {
			log.Tracef("Skipping scenario '%s' because it does not match any stage paths", scenario.Name())
			continue
		}

		collected++
		fmt.Println(scenario.Name())
	}

	log.Infof("Selected %d scenarios", collected)
	return nil
}
