package cmd

import (
	"encoding/json"
	"fmt"
	"tridenttools/cmd/mkcosi/cosi"

	log "github.com/sirupsen/logrus"
)

type ReadMetadata struct {
	Source string `arg:"" help:"Path to the COSI file to read metadata from." type:"existingfile" required:""`
}

func (b *ReadMetadata) Run() error {
	log.WithField("source", b.Source).Info("Reading metadata from COSI file")
	cosiMetadata, err := cosi.ScanCosiMetadataFromFile(b.Source)
	if err != nil {
		return fmt.Errorf("failed to read metadata from COSI file: %w", err)
	}

	log.Info("COSI file parsed successfully")

	marshalled, err := json.MarshalIndent(cosiMetadata, "", "    ")
	if err != nil {
		return fmt.Errorf("failed to marshal metadata: %w", err)
	}

	fmt.Println(string(marshalled))

	return nil
}
