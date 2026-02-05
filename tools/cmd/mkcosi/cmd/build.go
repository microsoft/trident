package cmd

type BuildCmd struct {
	Source string `arg:"" help:"Source image to build COSI from." required:"" type:"existingfile"`
	Output string `arg:"" help:"Output file to write COSI to." required:"" type:"path"`
	Arch   string `short:"a" help:"Architecture to build for" default:"x86_64" enum:"arm64,x86_64"`
}

func (r *BuildCmd) Run() error {
	return nil
}
