package cli

import (
	"os"

	"storm/internal/cli/list"
	"storm/internal/cli/run"

	"github.com/alecthomas/kong"
	log "github.com/sirupsen/logrus"
)

type GlobalOpts struct {
	Verbosity   log.Level `short:"v" help:"Set log level" default:"info"`
	AzureDevops bool      `short:"a" help:"Enable Azure DevOps integration" env:"TF_BUILD"`
}

type cli struct {
	Global GlobalOpts      `embed:""`
	List   list.ListCmd    `cmd:"" help:"List resources"`
	Run    run.ScenarioCmd `cmd:"" help:"Run a specific scenario"`
	Helper run.HelperCmd   `cmd:"" help:"Run a specific helper"`
}

func ParseCommandLine(name string) (*kong.Context, GlobalOpts) {
	// Force display help if no arguments are provided
	if len(os.Args) < 2 {
		os.Args = append(os.Args, "--help")
	}

	cli := cli{}
	ctx := kong.Parse(&cli, kong.Name(name))
	return ctx, cli.Global
}
