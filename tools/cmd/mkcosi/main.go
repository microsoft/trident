package main

import (
	"tridenttools/cmd/mkcosi/cmd"

	"github.com/alecthomas/kong"
	log "github.com/sirupsen/logrus"
)

type CLI struct {
	AddVpc          cmd.AddVpcCmd       `cmd:"" help:"Add a VPC footer to a COSI file."`
	Build           cmd.BuildCmd        `cmd:"" help:"Build a COSI file from a raw or fixed-vhd image."`
	DeleteFs        cmd.DeleteFs        `cmd:"" help:"Delete the specified filesystems from a COSI file."`
	InsertTemplate  cmd.InsertTemplate  `cmd:"" help:"Insert a Host Configuration template into a COSI file."`
	RandomizeFsUuid cmd.RandomizeFsUuid `cmd:"" help:"Randomize the UUID of the specified filesystems in a COSI file."`
	ReadMetadata    cmd.ReadMetadata    `cmd:"" help:"Read metadata from a COSI file."`
	Serve           cmd.ServeCmd        `cmd:"" help:"Serve a COSI file over HTTP."`
	ForceColor      bool                `help:"Force color output." short:"c"`
	Trace           bool                `help:"Enable trace logging." short:"t"`
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
