package list

import (
	"fmt"
	"slices"
	"storm/pkg/storm/core"
)

type ListTagsCmd struct {
}

func (cmd *ListTagsCmd) Run(suite core.SuiteContext) error {
	log := suite.Logger()
	log.Info("Listing all tags")

	// Create a map to store the tags
	var tags_set map[string]bool = make(map[string]bool)

	for _, scenario := range suite.Scenarios() {
		for _, tag := range scenario.Tags() {
			tags_set[tag] = true
		}
	}

	// Sort the tags
	var tags []string
	for tag := range tags_set {
		tags = append(tags, tag)
	}

	slices.Sort(tags)

	for _, tag := range tags {
		fmt.Println(tag)
	}
	return nil
}
