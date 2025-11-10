package cmd

import (
	"fmt"
	"os"
	"tridenttools/cmd/mkcosi/builder"
	"tridenttools/cmd/mkcosi/cosi"
	"tridenttools/cmd/mkcosi/metadata"

	log "github.com/sirupsen/logrus"
)

const HostConfigurationTemplateFilename = "hostConfigurationTemplate.yaml"

type InsertTemplate struct {
	Source   string `arg:"" help:"Path to the COSI file to read metadata from." type:"existingfile" required:""`
	Output   string `arg:"" help:"Path to write the new COSI file to." type:"path" required:""`
	Template string `arg:"" help:"Path to the host configuration template file to insert." type:"existingfile" required:""`
}

func (t *InsertTemplate) Run() error {
	log.WithField("source", t.Source).Info("Loading COSI file")
	log.WithField("template", t.Template).Info("Loading host configuration template file")

	// Read the COSI file once to get the metadata.
	cosi, err := cosi.ReadCosiFile(t.Source)
	if err != nil {
		return fmt.Errorf("failed to read file: %w", err)
	}
	defer cosi.Close()

	// Add the template to the COSI.
	sha384, err := builder.Sha384SumFile(t.Template)
	if err != nil {
		return fmt.Errorf("failed to calculate sha384 of %s: %w", t.Template, err)
	}
	cosi.Metadata.HostConfigurationTemplate = &metadata.AuxillaryFile{
		Path:       HostConfigurationTemplateFilename,
		Sha384:     sha384,
		SourceFile: t.Template,
	}

	// Write the new COSI file.
	outFile, err := os.Create(t.Output)
	if err != nil {
		return fmt.Errorf("failed to create output file: %w", err)
	}
	defer outFile.Close()

	err = builder.BuildCosi(outFile, &cosi.Metadata)
	if err != nil {
		return fmt.Errorf("failed to build COSI file: %w", err)
	}

	return nil
}
