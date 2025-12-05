package cmd

import (
	"fmt"
	"os"
	"tridenttools/cmd/mkcosi/builder"
	"tridenttools/cmd/mkcosi/cosi"

	log "github.com/sirupsen/logrus"
)

const HostConfigurationTemplateFilename = "hostConfigurationTemplate.yaml"

type InsertTemplate struct {
	Source   string `arg:"" help:"Path to the COSI file to read metadata from." type:"existingfile" required:""`
	Output   string `arg:"" help:"Path to write the new COSI file to." type:"path" required:""`
	Template string `arg:"" help:"Path to the Host Configuration template file to insert." type:"existingfile" required:""`
}

func (t *InsertTemplate) Run() error {
	log.WithField("source", t.Source).Info("Loading COSI file")
	log.WithField("template", t.Template).Info("Loading Host Configuration template file")

	// Read the COSI file once to get the metadata.
	cosi, err := cosi.ReadCosiFile(t.Source)
	if err != nil {
		return fmt.Errorf("failed to read file: %w", err)
	}
	defer cosi.Close()

	// Read the template file
	template, err := os.ReadFile(t.Template)
	if err != nil {
		return fmt.Errorf("failed to read template file: %w", err)
	}
	cosi.Metadata.HostConfigurationTemplate = string(template)

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
