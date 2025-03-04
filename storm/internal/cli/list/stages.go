package list

import (
	"encoding/json"
	"fmt"
	"slices"
	"storm/pkg/storm/core"
	"storm/pkg/storm/utils"

	"github.com/sirupsen/logrus"
)

type ListStagePathsCmd struct {
	Json   bool     `short:"j" long:"json" help:"Output in JSON format"`
	Filter []string `short:"f" long:"filter" description:"Filter stage paths by a common root"`
}

func (cmd *ListStagePathsCmd) Run(suite core.SuiteContext) error {
	log := suite.Logger()
	log.Info("Listing stage paths")

	pathFilter := utils.NewPathFilterFromSlice(cmd.Filter, true)

	var allStagePaths []string
	for _, scenario := range suite.Scenarios() {
		for _, stagePath := range scenario.StagePaths() {
			if pathFilter.Match(stagePath) {
				allStagePaths = append(allStagePaths, stagePath)
			}
		}
	}

	if cmd.Json {
		outputStagesAsJson(allStagePaths, log)
	} else {
		outputStagesAsList(allStagePaths)
	}

	return nil
}

func outputStagesAsList(allStagePaths []string) {
	// Create a map to store the tags
	var stagePathsSset map[string]bool = make(map[string]bool)

	for _, stagePath := range allStagePaths {
		stagePathsSset[stagePath] = true
	}

	// Sort the tags
	var stagePaths []string
	for stagePath := range stagePathsSset {
		stagePaths = append(stagePaths, stagePath)
	}

	slices.Sort(stagePaths)

	for _, stagePath := range stagePaths {
		fmt.Println(stagePath)
	}
}

func outputStagesAsJson(allStagePaths []string, log *logrus.Logger) {
	tree := utils.NewPathTree()
	for _, stagePath := range allStagePaths {
		tree.Add(stagePath)
	}

	fmt.Printf("Tee: %v\n", tree)

	data, err := json.MarshalIndent(tree, "", "  ")
	if err != nil {
		log.Fatalf("Failed to marshal stage paths to JSON: %v", err)
	}

	fmt.Println(string(data))
}
