package metadata

import (
	"archive/tar"
	"encoding/json"
	"fmt"
	"io"
	"os"

	log "github.com/sirupsen/logrus"
)

type ReadMetadata struct {
	Source string `arg:"" help:"Path to the COSI file to read metadata from." type:"existingfile" required:""`
}

func (b *ReadMetadata) Run() error {
	log.WithField("source", b.Source).Info("Reading metadata from COSI file")
	source, err := os.Open(b.Source)
	if err != nil {
		return fmt.Errorf("failed to open COSI file: %w", err)
	}
	reader := tar.NewReader(source)

	var metadataFound bool
	var index int = 0

	for ; ; index++ {
		header, err := reader.Next()
		if err != nil {
			if err == io.EOF {
				break
			} else {
				return fmt.Errorf("failed to read next entry in COSI file: %w", err)
			}
		}

		log.WithField("name", header.Name).WithField("index", index).WithField("size", header.Size).Info("Found entry in COSI file")

		if header.Name == "metadata.json" {
			err = printMetadata(reader)
			if err != nil {
				return fmt.Errorf("failed to print metadata: %w", err)
			}
			metadataFound = true
		}
	}

	log.WithField("entries", index).Info("End of COSI file")

	if !metadataFound {
		return fmt.Errorf("metadata.json not found in COSI file")
	}

	return nil
}

func printMetadata(reader io.Reader) error {
	var metadata MetadataJson
	raw_metadata, err := io.ReadAll(reader)
	if err != nil {
		return fmt.Errorf("failed to read metadata: %w", err)
	}

	err = json.Unmarshal(raw_metadata, &metadata)
	if err != nil {
		return fmt.Errorf("failed to unmarshal metadata: %w", err)
	}

	out, err := json.MarshalIndent(metadata, "", "    ")
	if err != nil {
		return fmt.Errorf("failed to marshal metadata: %w", err)
	}

	fmt.Println(string(out))

	return nil
}
