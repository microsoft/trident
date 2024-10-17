package main

import (
	"argus_toolkit/cmd/mkcosi/variants"

	"github.com/alecthomas/kong"
	log "github.com/sirupsen/logrus"
)

type CLI struct {
	Build BuildCmd `cmd:"" help:"Build an COSI file from existing test images!"`
}

type BuildCmd struct {
	Regular variants.BuildRegular `cmd:"" help:"Build a regular COSI"`
	Verity  variants.BuildVerity  `cmd:"" help:"Build a verity COSI"`
}

func main() {
	log.SetLevel(log.DebugLevel)
	log.Debug("Starting mkcosi")
	cli := CLI{}
	ctx := kong.Parse(&cli)
	err := ctx.Run()
	ctx.FatalIfErrorf(err)
}

func (b *BuildCmd) Run() error {
	return nil
}
