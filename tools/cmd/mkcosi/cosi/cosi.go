package cosi

import (
	"argus_toolkit/cmd/mkcosi/metadata"
	"fmt"
	"io"
	"os"
	"path/filepath"

	log "github.com/sirupsen/logrus"
)

type CosiEntry struct {
	Name   string
	Size   int64
	Offset int64
}

type Cosi struct {
	Metadata metadata.MetadataJson
	tmpDir   string
}

func (c *Cosi) Close() error {
	log.WithField("location", c.tmpDir).Debug("Removing temporary directory")
	err := os.RemoveAll(c.tmpDir)
	if err != nil {
		return fmt.Errorf("failed to remove temporary directory '%s': %w", c.tmpDir, err)
	}

	return nil
}

func ReadCosiFile(filepath string) (*Cosi, error) {
	file, err := os.Open(filepath)
	if err != nil {
		return nil, fmt.Errorf("failed to open COSI file '%s': %w", filepath, err)
	}

	cosi, err := ReadCosi(file)
	if err != nil {
		return nil, fmt.Errorf("failed to read COSI file '%s': %w", filepath, err)
	}

	return cosi, nil
}

func ReadCosi(reader io.ReadSeekCloser) (*Cosi, error) {
	dir, contents, err := extractCosiFile(reader)
	if err != nil {
		return nil, fmt.Errorf("failed to extract COSI file: %w", err)
	}

	metadata_location := filepath.Join(dir, "metadata.json")
	metadata_file, err := os.Open(metadata_location)
	if err != nil {
		os.RemoveAll(dir)
		return nil, fmt.Errorf("failed to open metadata file '%s': %w", metadata_location, err)
	}

	metadata, err := readMetadataFile(metadata_file)
	if err != nil {
		os.RemoveAll(dir)
		return nil, fmt.Errorf("failed to read metadata file '%s': %w", metadata_location, err)
	}

	err = validateMetadata(metadata, contents)
	if err != nil {
		os.RemoveAll(dir)
		return nil, fmt.Errorf("failed to validate metadata: %w", err)
	}

	// Because we extracted the files from the COSI file as they are, we can
	// populate the SourceFile as the path inside the tarball appended to the
	// extraction location.
	for i := range metadata.Images {
		img := &metadata.Images[i]
		img.Image.SourceFile = filepath.Join(dir, img.Image.Path)
		if img.Verity != nil {
			img.Verity.Image.SourceFile = filepath.Join(dir, img.Verity.Image.Path)
		}
	}

	cosi := &Cosi{
		Metadata: *metadata,
		tmpDir:   dir,
	}

	return cosi, nil
}

func (c *Cosi) ImageForMountPoint(mountPoint string) *metadata.Image {
	for _, fs := range c.Metadata.Images {
		if fs.MountPoint == mountPoint {
			return &fs
		}
	}

	return nil
}
