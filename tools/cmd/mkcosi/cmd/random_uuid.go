package cmd

import (
	"argus_toolkit/cmd/mkcosi/builder"
	"argus_toolkit/cmd/mkcosi/cosi"
	"argus_toolkit/cmd/mkcosi/metadata"
	"fmt"
	"io"
	"os"
	"os/exec"
	"slices"
	"strings"

	"github.com/google/uuid"
	"github.com/klauspost/compress/zstd"
	log "github.com/sirupsen/logrus"
)

type RandomizeFsUuid struct {
	Source      string   `arg:"" help:"Path to the COSI file to read metadata from." type:"existingfile" required:""`
	Output      string   `arg:"" help:"Path to write the new COSI file to." type:"path" required:""`
	FsMntPoints []string `arg:"" help:"Mount point(s) of the target filesystem(s)." required:""`
}

func (r *RandomizeFsUuid) Run() error {
	log.WithField("source", r.Source).Info("Reading metadata from COSI file")
	// Read the cosi file once to get the metadata.
	cosi, err := cosi.ReadCosiFile(r.Source)
	if err != nil {
		return fmt.Errorf("failed to read file: %w", err)
	}
	defer cosi.Close()

	// Check that the requested filesystems exist and are ext* filesystems.
	for _, fsMntPoint := range r.FsMntPoints {
		fsImg := cosi.ImageForMountPoint(fsMntPoint)
		if fsImg == nil {
			return fmt.Errorf("filesystem with mount point '%s' not found in COSI file", fsMntPoint)
		}

		if !strings.HasPrefix(fsImg.FsType, "ext") {
			return fmt.Errorf("filesystem with mount point '%s' is not an ext* filesystem", fsMntPoint)
		}
	}

	// Create a temporary directory to uncompress partition images.
	tmpDir, err := os.MkdirTemp("", "cosi-decompress-*")
	if err != nil {
		return fmt.Errorf("failed to create temporary directory: %w", err)
	}
	defer os.RemoveAll(tmpDir)

	// Randomize the UUIDs of the filesystems.
	for i := range len(cosi.Metadata.Images) {
		img := &cosi.Metadata.Images[i]
		log.WithField("mount_point", img.MountPoint).Info("Processing filesystem")
		if slices.Contains(r.FsMntPoints, img.MountPoint) {
			// We must randomize this filesystem's UUID.
			err = randomizeFsUuid(img, tmpDir)
			if err != nil {
				return fmt.Errorf("failed to randomize UUID for filesystem with mount point '%s': %w", img.MountPoint, err)
			}
		} else {
			// We can skip this filesystem.
			continue
		}
	}

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

	return nil
}

// Takes in a filesystem image and randomizes its UUID.
//
// It updates the filesystem UUID in the passed image metadata and updates the
// image file metadata to reflect changes to the compressed and uncompressed
// image.
func randomizeFsUuid(img *metadata.Image, tempDir string) error {
	log.WithField("mount_point", img.MountPoint).Info("Randomizing UUID for filesystem")
	if img.Image.SourceFile == "" {
		return fmt.Errorf("filesystem image source file not found")
	}

	// Open the image file.
	decompressedImg, err := builder.DecompressImage(img.Image.SourceFile)
	if err != nil {
		return fmt.Errorf("failed to decompress image: %w", err)
	}
	defer decompressedImg.Close()
	defer os.Remove(decompressedImg.Name())

	// Create new UUID for the filesystem.
	new_uuid := uuid.New()
	img.FsUuid = new_uuid.String()
	log.WithField("new_uuid", new_uuid).Info("Randomized UUID for filesystem")

	// Set the new UUID in the filesystem.
	log.Trace("Setting new UUID for filesystem")
	err = setFsUuid(decompressedImg.Name(), new_uuid)
	if err != nil {
		return fmt.Errorf("failed to set UUID for filesystem: %w", err)
	}

	// Go to the start of the temporary file for re-compression.
	log.Trace("Setting new UUID for filesystem")
	_, err = decompressedImg.Seek(0, 0)
	if err != nil {
		return fmt.Errorf("failed to seek to start of temporary file: %w", err)
	}

	// Get the size of the decompressed image.
	log.Trace("Getting filesystem image file stats")
	decompressedImgStat, err := decompressedImg.Stat()
	if err != nil {
		return fmt.Errorf("failed to get filesystem image file stats: %w", err)
	}

	// Update metadata with the new decompressed image size.
	log.WithField("UncompressedSize", decompressedImgStat.Size()).Trace("Updating metadata with new decompressed image size")
	img.Image.UncompressedSize = uint64(decompressedImgStat.Size())

	// Create a temporary file to store the re-compressed image.
	log.Trace("Creating temporary file for re-compressed image")
	tmpFile2, err := os.CreateTemp(tempDir, "fs-image-*")
	if err != nil {
		return fmt.Errorf("failed to create temporary file: %w", err)
	}
	defer tmpFile2.Close()

	// Update metadata with the new source image.
	log.WithField("source", tmpFile2.Name()).Trace("Updating metadata with new source image")
	img.Image.SourceFile = tmpFile2.Name()

	// Create a compressor for the image.
	log.Trace("Creating zstd writer for re-compressed image")
	zstdWriter, err := zstd.NewWriter(tmpFile2, zstd.WithEncoderLevel(zstd.SpeedBetterCompression))
	if err != nil {
		return fmt.Errorf("failed to create zstd writer: %w", err)
	}
	defer zstdWriter.Close()

	// Re-compress the image.
	log.Trace("Re-compressing filesystem image")
	_, err = io.Copy(zstdWriter, decompressedImg)
	if err != nil {
		return fmt.Errorf("failed to re-compress filesystem image: %w", err)
	}

	// Close everything to flush all data.
	zstdWriter.Close()
	decompressedImg.Close()
	tmpFile2.Close()

	// Re-open the file
	tmpFile2, err = os.Open(tmpFile2.Name())
	if err != nil {
		return fmt.Errorf("failed to re-open re-compressed image: %w", err)
	}
	defer tmpFile2.Close()

	// Get the size of the re-compressed image.
	log.Trace("Getting re-compressed filesystem image file stats")
	tmpFile2Stat, err := tmpFile2.Stat()
	if err != nil {
		return fmt.Errorf("failed to get re-compressed filesystem image file stats: %w", err)
	}

	// Update metadata with the new compressed image size.
	log.WithField("CompressedSize", tmpFile2Stat.Size()).Trace("Updating metadata with new compressed image size")
	img.Image.CompressedSize = uint64(tmpFile2Stat.Size())

	// Update metadata with the new SHA384 hash of the compressed image.
	log.Trace("Calculating SHA384 hash of re-compressed image")
	_, err = tmpFile2.Seek(0, 0)
	if err != nil {
		return fmt.Errorf("failed to seek to start of re-compressed image: %w", err)
	}

	img.Image.Sha384, err = builder.Sha384SumReader(tmpFile2)
	if err != nil {
		return fmt.Errorf("failed to calculate SHA384 hash of re-compressed image: %w", err)
	}

	return nil
}

func setFsUuid(imagePath string, newUuid uuid.UUID) error {
	out, err := exec.Command("e2fsck", "-f", "-y", imagePath).CombinedOutput()
	if err != nil {
		log.WithField("output", string(out)).Error("Failed to check filesystem")
		return fmt.Errorf("failed to check filesystem: %w", err)
	}
	out, err = exec.Command("tune2fs", "-U", newUuid.String(), imagePath).CombinedOutput()
	if err != nil {
		log.WithField("output", string(out)).Error("Failed to set UUID for filesystem")
		return fmt.Errorf("failed to set UUID for filesystem: %w", err)
	}

	return nil
}
