package main

import (
	"tridenttools/cmd/mkcosi/builder"
	"tridenttools/cmd/mkcosi/cmd"

	"github.com/alecthomas/kong"
	log "github.com/sirupsen/logrus"
)

type CLI struct {
	Build           BuildCmd            `cmd:"" help:"Build a COSI file from existing test images!"`
	ReadMetadata    cmd.ReadMetadata    `cmd:"" help:"Read metadata from a COSI file."`
	RandomizeFsUuid cmd.RandomizeFsUuid `cmd:"" help:"Randomize the UUID of the specified filesystems in a COSI file."`
	DeleteFs        cmd.DeleteFs        `cmd:"" help:"Delete the specified filesystems from a COSI file."`
	ForceColor      bool                `help:"Force color output." short:"c"`
	Trace           bool                `help:"Enable trace logging." short:"t"`
}

type BuildCmd struct {
	Regular builder.BuildRegular `cmd:"" help:"Build a regular COSI"`
	Verity  builder.BuildVerity  `cmd:"" help:"Build a verity COSI"`
}

func main() {
	log.SetLevel(log.DebugLevel)
	log.Debug("Starting mkcosi")
	cli := CLI{}
	ctx := kong.Parse(&cli)

	if cli.Trace {
		log.SetLevel(log.TraceLevel)
	}

	if cli.ForceColor {
		log.SetFormatter(&log.TextFormatter{
			ForceColors: true,
		})
	}

	err := ctx.Run()
	ctx.FatalIfErrorf(err)
}

func (b *BuildCmd) Run() error {
	return nil
}
