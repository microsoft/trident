package cmd

import (
	"fmt"
	"os"
	"slices"
	"tridenttools/cmd/mkcosi/builder"
	"tridenttools/cmd/mkcosi/cosi"
	"tridenttools/cmd/mkcosi/metadata"

	log "github.com/sirupsen/logrus"
)

type DeleteFs struct {
	Source      string   `arg:"" help:"Path to the COSI file to read metadata from." type:"existingfile" required:""`
	Output      string   `arg:"" help:"Path to write the new COSI file to." type:"path" required:""`
	FsMntPoints []string `arg:"" help:"Mount point(s) of the filesystem(s) to delete." required:""`
}

func (r *DeleteFs) Run() error {
	log.WithField("source", r.Source).Info("Reading metadata from COSI file")
	// Read the cosi file once to get the metadata.
	cosi, err := cosi.ReadCosiFile(r.Source)
	if err != nil {
		return fmt.Errorf("failed to read file: %w", err)
	}
	defer cosi.Close()

	// First check that the requested filesystems exist.
	for _, fsMntPoint := range r.FsMntPoints {
		fsImg := cosi.ImageForMountPoint(fsMntPoint)
		if fsImg == nil {
			return fmt.Errorf("filesystem with mount point '%s' not found in COSI file", fsMntPoint)
		}
	}

	// Create a new list with images to KEEP.
	keepImages := make([]metadata.Image, 0, len(cosi.Metadata.Images))
	for _, img := range cosi.Metadata.Images {
		if slices.Contains(r.FsMntPoints, img.MountPoint) {
			log.WithField("mountpoint", img.MountPoint).Info("Deleting filesystem from COSI")
			continue
		}
		keepImages = append(keepImages, img)
	}

	// Update the metadata to only include the kept images.
	cosi.Metadata.Images = keepImages

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
