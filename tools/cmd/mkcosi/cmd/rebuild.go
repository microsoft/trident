package cmd

import (
	"fmt"
	"os"
	"tridenttools/cmd/mkcosi/builder"
	"tridenttools/cmd/mkcosi/cosi"

	log "github.com/sirupsen/logrus"
)

type RebuildCmd struct {
	Source string `arg:"" help:"Path to the COSI file to read metadata from." type:"existingfile" required:""`
	Output string `arg:"" help:"Path to write the new COSI file to." type:"path" required:""`
}

func (r *RebuildCmd) Run() error {
	log.WithField("source", r.Source).Info("Reading metadata from COSI file")
	// Read the cosi file once to get the metadata.
	cosi, err := cosi.ReadCosiFile(r.Source)
	if err != nil {
		return fmt.Errorf("failed to read file: %w", err)
	}
	defer cosi.Close()

	// Write the new cosi file.
	outFile, err := os.Create(r.Output)
	if err != nil {
		return fmt.Errorf("failed to create output file: %w", err)
	}
	defer outFile.Close()

	err = builder.BuildCosi(outFile, &cosi.Metadata)
	if err != nil {
		return fmt.Errorf("failed to build COSI file: %w", err)
	}

	log.WithField("output", r.Output).Info("Successfully wrote new COSI file")

	return nil
}
