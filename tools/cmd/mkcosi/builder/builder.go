package builder

import (
	"archive/tar"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"

	"tridenttools/cmd/mkcosi/metadata"

	log "github.com/sirupsen/logrus"
)

// Builds a COSI file from the given metadata, outputting the result to the
// provided writer.
//
// The Metadata is expected to have the internal field SourceFile set for each
// image. This is used to read the image data from the source file and add it to
// the COSI file.
func BuildCosi(output io.Writer, cosiMetadata *metadata.MetadataJson) error {
	tw := tar.NewWriter(output)
	defer tw.Close()

	// Pre-calculate offsets
	const tarHeaderSize uint64 = 512
	const tarBlockSize uint64 = 512

	// Start the offset after the tar header of the first image file
	var currentOffset uint64 = tarHeaderSize

	// Helper function to move the offset forward based on an image file
	moveOffsetWithImage := func(img *metadata.ImageFile) {
		// Set the offset for this image
		img.Offset = currentOffset

		log.WithField("path", img.Path).WithField(
			"offset",
			fmt.Sprintf("%d[0x%X]", img.Offset, img.Offset),
		).Debug("Set image offset")

		// Now move the offset forward by the size of the image file, rounded up to
		// the next tar block
		blocks := (img.CompressedSize + tarBlockSize - 1) / tarBlockSize
		currentOffset += blocks * tarBlockSize

		// Add the size of the tar header for the next image
		currentOffset += tarHeaderSize
	}

	// Iterate over all images to set their offsets
	for i := range cosiMetadata.Images {
		img := &cosiMetadata.Images[i]
		moveOffsetWithImage(&img.Image)

		// If there is a verity image, set its offset as well
		if img.Verity != nil {
			moveOffsetWithImage(&img.Verity.Image)
		}
	}

	marshalledMetadata, err := json.MarshalIndent(cosiMetadata, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal metadata: %w", err)
	}

	err = addFile(tw, "metadata.json", uint64(len(marshalledMetadata)), bytes.NewReader(marshalledMetadata))
	if err != nil {
		return fmt.Errorf("failed to add metadata file: %w", err)
	}

	for _, img := range cosiMetadata.Images {

		err = addImage(tw, &img.Image)
		if err != nil {
			return fmt.Errorf("failed to add image file: %w", err)
		}

		if img.Verity != nil {
			err = addImage(tw, &img.Verity.Image)
			if err != nil {
				return fmt.Errorf("failed to add verity file: %w", err)
			}
		}
	}

	return nil
}

func addImage(tw *tar.Writer, img *metadata.ImageFile) error {
	if img.SourceFile == "" {
		return fmt.Errorf("source file not set")
	}

	file, err := os.Open(img.SourceFile)
	if err != nil {
		return fmt.Errorf("failed to open source file: %w", err)
	}
	defer file.Close()

	log.WithField("source", img.SourceFile).WithField("path", img.Path).WithField("size", img.CompressedSize).Debug("Adding image file to COSI")
	err = addFile(tw, img.Path, img.CompressedSize, file)
	if err != nil {
		return fmt.Errorf("failed to add image file: %w", err)
	}

	return nil
}

func addFile(tw *tar.Writer, name string, size uint64, content io.Reader) error {
	err := tw.WriteHeader(&tar.Header{
		Typeflag: tar.TypeReg,
		Name:     name,
		Size:     int64(size),
		Mode:     0o400,
		Format:   tar.FormatPAX,
	})
	if err != nil {
		return fmt.Errorf("failed to write tar header for '%s': %w", name, err)
	}

	_, err = io.Copy(tw, content)
	if err != nil {
		return fmt.Errorf("failed to copy file to tar: %w", err)
	}

	return nil
}
