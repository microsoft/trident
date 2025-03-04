package list

type ListCmd struct {
	Scenarios  ListScenariosCmd  `cmd:"" help:"List available scenarios"`
	Tags       ListTagsCmd       `cmd:"" help:"List all tags"`
	StagePaths ListStagePathsCmd `cmd:"" help:"List all stage paths"`
	Helpers    ListHelpersCmd    `cmd:"" help:"List all helpers"`
}
