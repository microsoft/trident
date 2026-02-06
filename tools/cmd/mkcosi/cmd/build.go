package cmd

import (
	"fmt"
	"os"
	"tridenttools/cmd/mkcosi/builder"
	"tridenttools/cmd/mkcosi/generator"
	"tridenttools/cmd/mkcosi/metadata"
)

type BuildCmd struct {
	Source string `arg:"" help:"Source image to build COSI from." required:"" type:"existingfile"`
	Output string `arg:"" help:"Output file to write COSI to." required:"" type:"path"`
	Arch   string `short:"a" help:"Architecture to build for" default:"x86_64" enum:"arm64,x86_64"`
}

func (r *BuildCmd) Run() error {
	cosi, err := generator.CosiFromImage(r.Source, metadata.OsArchitecture(r.Arch))
	if err != nil {
		return fmt.Errorf("failed to create COSI metadata from image '%s': %w", r.Source, err)
	}
	defer cosi.Close()

	outFile, err := os.Create(r.Output)
	if err != nil {
		return fmt.Errorf("failed to create output file '%s': %w", r.Output, err)
	}
	defer outFile.Close()

	err = builder.BuildCosi(outFile, &cosi.Metadata)
	if err != nil {
		return fmt.Errorf("failed to build COSI file: %w", err)
	}

	return nil
}
