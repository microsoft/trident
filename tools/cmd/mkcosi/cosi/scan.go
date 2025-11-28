package cosi

import (
	"archive/tar"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"tridenttools/cmd/mkcosi/metadata"

	log "github.com/sirupsen/logrus"
	"golang.org/x/exp/slices"
)

func ScanCosiMetadataFromFile(filepath string) (*metadata.MetadataJson, error) {
	file, err := os.Open(filepath)
	if err != nil {
		return nil, fmt.Errorf("failed to open COSI file '%s': %w", filepath, err)
	}

	data, err := ScanCosiMetadata(file)
	if err != nil {
		return nil, fmt.Errorf("failed to read metadata from COSI file '%s': %w", filepath, err)
	}

	return data, nil
}

func ScanCosiMetadata(source io.ReadSeeker) (*metadata.MetadataJson, error) {
	_, err := source.Seek(0, io.SeekStart)
	if err != nil {
		return nil, fmt.Errorf("failed to seek to start of COSI file: %w", err)
	}

	var cosiMetadata *metadata.MetadataJson
	images := make([]string, 0)

	reader := tar.NewReader(source)

	for index := 0; ; index++ {
		header, err := reader.Next()
		if err != nil {
			if err == io.EOF {
				break
			} else {
				return nil, fmt.Errorf("failed to read next entry in COSI file: %w", err)
			}
		}

		log.WithField("name", header.Name).WithField("index", index).WithField("size", header.Size).Info("Found entry in COSI file")

		if header.Name == "metadata.json" {
			cosiMetadata, err = readMetadataFile(reader)
			if err != nil {
				return nil, fmt.Errorf("failed to read metadata from COSI file: %w", err)
			}
		} else {
			images = append(images, header.Name)
		}
	}

	if cosiMetadata == nil {
		return nil, fmt.Errorf("metadata.json not found in COSI file")
	}

	err = validateMetadata(cosiMetadata, images)
	if err != nil {
		return nil, fmt.Errorf("failed to validate metadata: %w", err)
	}

	return cosiMetadata, nil
}

func readMetadataFile(reader io.Reader) (*metadata.MetadataJson, error) {
	raw_metadata, err := io.ReadAll(reader)
	if err != nil {
		return nil, fmt.Errorf("failed to read metadata from file: %w", err)
	}

	var metadata metadata.MetadataJson
	err = json.Unmarshal(raw_metadata, &metadata)
	if err != nil {
		return nil, fmt.Errorf("failed to unmarshal metadata: %w", err)
	}

	return &metadata, nil
}

func validateMetadata(metadata *metadata.MetadataJson, images []string) error {
	for _, metadataImg := range metadata.Images {
		if !slices.Contains(images, metadataImg.Image.Path) {
			return fmt.Errorf("image '%s' in COSI metadata not found in COSI file", metadataImg.Image.Path)
		}
	}

	return nil
}
