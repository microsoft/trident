// Package generator provides functionality for generating COSI files from disk images.
package generator

import (
	"bytes"
	"crypto/sha512"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"

	"tridenttools/cmd/mkcosi/cosi"
	"tridenttools/cmd/mkcosi/gpt"
	"tridenttools/cmd/mkcosi/metadata"

	"github.com/google/uuid"
	"github.com/klauspost/compress/zstd"
	rpmdb "github.com/knqyf263/go-rpmdb/pkg"
	log "github.com/sirupsen/logrus"
	_ "modernc.org/sqlite" // Register SQLite driver for go-rpmdb
)

const (
	// DefaultZstdWindowLog is the default zstd window log (power of 2) for compression.
	DefaultZstdWindowLog = 22 // 4 MiB window
	// VHDFooterSize is the size of a VHD footer in bytes.
	VHDFooterSize = 512
)

// isVHDFixed checks if the file has a VHD fixed footer by looking for the
// "conectix" signature in the last 512 bytes.
func isVHDFixed(file *os.File) (bool, error) {
	stat, err := file.Stat()
	if err != nil {
		return false, fmt.Errorf("failed to stat file: %w", err)
	}

	if stat.Size() < 512 {
		return false, nil
	}

	// Read the last 512 bytes
	footer := make([]byte, 512)
	_, err = file.ReadAt(footer, stat.Size()-512)
	if err != nil {
		return false, fmt.Errorf("failed to read potential VHD footer: %w", err)
	}

	// Check for "conectix" signature
	return bytes.Equal(footer[:8], []byte("conectix")), nil
}

// partitionInfo holds information about a partition during processing.
type partitionInfo struct {
	entry      gpt.PartitionEntry
	imageFile  *metadata.ImageFile
	partNumber uint32
	fsType     string
	fsUuid     string
	mountPoint string
}

// CosiFromImage creates a COSI structure from a raw or fixed VHD disk image.
// The image must contain a valid GPT partition table and use GRUB as the bootloader.
// Returns a Cosi object with all metadata populated and compressed images in a temporary directory.
// The caller is responsible for calling Close() on the returned Cosi to clean up the temporary directory.
func CosiFromImage(imagePath string, arch metadata.OsArchitecture) (*cosi.Cosi, error) {
	// Open the image file
	file, err := os.Open(imagePath)
	if err != nil {
		return nil, fmt.Errorf("failed to open image file: %w", err)
	}
	defer file.Close()

	// Get file stat for size
	stat, err := file.Stat()
	if err != nil {
		return nil, fmt.Errorf("failed to stat image file: %w", err)
	}
	totalFileSize := stat.Size()

	// Detect if this is a VHD fixed image
	isVHD, err := isVHDFixed(file)
	if err != nil {
		return nil, fmt.Errorf("failed to detect VHD footer: %w", err)
	}

	var diskSize uint64
	var vhdFooterOffset int64
	if isVHD {
		log.Info("Detected fixed VHD image, ignoring last 512 bytes")
		vhdFooterOffset = VHDFooterSize
		diskSize = uint64(totalFileSize - vhdFooterOffset)
	} else {
		diskSize = uint64(totalFileSize)
	}

	// Parse the GPT
	parsedGPT, err := gpt.ParseGPT(file, diskSize)
	if err != nil {
		return nil, fmt.Errorf("failed to parse GPT: %w", err)
	}

	log.WithField("partitions", len(parsedGPT.Partitions)).Info("Parsed GPT successfully")

	// Create temporary directory for extracted images
	tmpDir, err := os.MkdirTemp("", "mkcosi-generator-*")
	if err != nil {
		return nil, fmt.Errorf("failed to create temporary directory: %w", err)
	}

	// Cleanup function in case of error
	cleanup := func() {
		log.WithField("location", tmpDir).Debug("Cleaning up temporary directory due to error")
		os.RemoveAll(tmpDir)
	}

	// Create images subdirectory
	imagesDir := filepath.Join(tmpDir, "images")
	if err := os.MkdirAll(imagesDir, 0755); err != nil {
		cleanup()
		return nil, fmt.Errorf("failed to create images directory: %w", err)
	}

	// Initialize metadata
	cosiMetadata := metadata.MetadataJson{
		Version: "1.2",
		Id:      uuid.New().String(),
		Disk: &metadata.Disk{
			Size:    diskSize,
			Type:    metadata.DiskTypeGpt,
			LbaSize: parsedGPT.LBASize,
		},
		Compression: &metadata.Compression{
			MaxWindowLog: DefaultZstdWindowLog,
		},
		Bootloader: metadata.Bootloader{
			Type: metadata.BootloaderTypeGrub,
		},
		OsArch: arch,
	}

	// Extract and compress the primary GPT region
	gptImageFile, err := extractAndCompressGPTRegion(file, parsedGPT, imagesDir)
	if err != nil {
		cleanup()
		return nil, fmt.Errorf("failed to extract GPT region: %w", err)
	}

	// Add primary-gpt region as the first entry
	cosiMetadata.Disk.GptRegions = append(cosiMetadata.Disk.GptRegions, metadata.GptDiskRegion{
		Image: *gptImageFile,
		Type:  metadata.RegionTypePrimaryGpt,
	})

	// Sort partitions by starting LBA to ensure physical order
	sortedPartitions := make([]gpt.PartitionEntry, len(parsedGPT.Partitions))
	copy(sortedPartitions, parsedGPT.Partitions)
	sort.Slice(sortedPartitions, func(i, j int) bool {
		return sortedPartitions[i].StartingLBA < sortedPartitions[j].StartingLBA
	})

	// Track mount points for filesystem scanning
	partitionInfos := make([]partitionInfo, 0, len(sortedPartitions))

	// Extract and compress each partition
	for i, partition := range sortedPartitions {
		partNumber := uint32(i + 1) // 1-based partition numbers

		log.WithFields(log.Fields{
			"partition": partNumber,
			"name":      partition.GetName(),
			"startLBA":  partition.StartingLBA,
			"endLBA":    partition.EndingLBA,
			"size":      partition.SizeInBytes(parsedGPT.LBASize),
		}).Info("Processing partition")

		imageFile, err := extractAndCompressPartition(file, &partition, parsedGPT.LBASize, imagesDir, partNumber, tmpDir)
		if err != nil {
			cleanup()
			return nil, fmt.Errorf("failed to extract partition %d: %w", partNumber, err)
		}

		// Add partition region
		pn := partNumber
		cosiMetadata.Disk.GptRegions = append(cosiMetadata.Disk.GptRegions, metadata.GptDiskRegion{
			Image:  *imageFile,
			Type:   metadata.RegionTypePartition,
			Number: &pn,
		})

		partitionInfos = append(partitionInfos, partitionInfo{
			entry:      partition,
			imageFile:  imageFile,
			partNumber: partNumber,
		})
	}

	// Now scan the filesystems to populate additional metadata
	// We need to decompress and mount each partition to get filesystem info
	err = populateFilesystemMetadata(&cosiMetadata, partitionInfos, tmpDir)
	if err != nil {
		cleanup()
		return nil, fmt.Errorf("failed to populate filesystem metadata: %w", err)
	}

	// Verify we have grub as bootloader
	if cosiMetadata.Bootloader.Type != metadata.BootloaderTypeGrub {
		cleanup()
		return nil, fmt.Errorf("only GRUB bootloader is supported, found: %s", cosiMetadata.Bootloader.Type)
	}

	// Create the Cosi object
	result := cosi.NewCosiWithTmpDir(cosiMetadata, tmpDir)

	return result, nil
}

// extractAndCompressGPTRegion extracts the primary GPT region and compresses it with zstd.
func extractAndCompressGPTRegion(file *os.File, parsedGPT *gpt.ParsedGPT, outputDir string) (*metadata.ImageFile, error) {
	// Extract the primary GPT region
	gptData, err := gpt.ExtractPrimaryGPTRegion(file, parsedGPT)
	if err != nil {
		return nil, fmt.Errorf("failed to extract GPT region: %w", err)
	}

	// Compress and write to file
	outputPath := filepath.Join(outputDir, "primary-gpt.raw.zst")
	compressedSize, sha384Hash, err := compressDataToFile(gptData, outputPath)
	if err != nil {
		return nil, fmt.Errorf("failed to compress GPT region: %w", err)
	}

	return &metadata.ImageFile{
		Path:             "images/primary-gpt.raw.zst",
		CompressedSize:   compressedSize,
		UncompressedSize: uint64(len(gptData)),
		Sha384:           sha384Hash,
		SourceFile:       outputPath,
	}, nil
}

// extractAndCompressPartition extracts a partition, optionally shrinks ext filesystems,
// and compresses it with zstd.
func extractAndCompressPartition(file *os.File, partition *gpt.PartitionEntry, lbaSize uint64, outputDir string, partNumber uint32, tmpDir string) (*metadata.ImageFile, error) {
	startOffset := partition.StartOffset(lbaSize)
	partitionSize := partition.SizeInBytes(lbaSize)

	// First, extract the raw partition to a temporary file
	rawPartPath := filepath.Join(tmpDir, fmt.Sprintf("partition-%d.raw", partNumber))
	err := extractRegionToFile(file, int64(startOffset), int64(partitionSize), rawPartPath)
	if err != nil {
		return nil, fmt.Errorf("failed to extract partition: %w", err)
	}
	defer os.Remove(rawPartPath)

	// Check if this is an ext filesystem and shrink it if possible
	shrunkSize, err := shrinkExtFilesystem(rawPartPath, partitionSize)
	if err != nil {
		log.WithError(err).WithField("partition", partNumber).Debug("Could not shrink filesystem, using full partition")
		shrunkSize = partitionSize
	}

	// Compress the (possibly shrunk) partition
	outputPath := filepath.Join(outputDir, fmt.Sprintf("partition-%d.raw.zst", partNumber))
	compressedSize, sha384Hash, err := compressFileRegionToFile(rawPartPath, int64(shrunkSize), outputPath)
	if err != nil {
		return nil, fmt.Errorf("failed to compress partition: %w", err)
	}

	return &metadata.ImageFile{
		Path:             fmt.Sprintf("images/partition-%d.raw.zst", partNumber),
		CompressedSize:   compressedSize,
		UncompressedSize: shrunkSize,
		Sha384:           sha384Hash,
		SourceFile:       outputPath,
	}, nil
}

// extractRegionToFile extracts a region from file to a new file.
func extractRegionToFile(file *os.File, offset int64, size int64, outputPath string) error {
	outputFile, err := os.Create(outputPath)
	if err != nil {
		return fmt.Errorf("failed to create output file: %w", err)
	}
	defer outputFile.Close()

	reader := io.NewSectionReader(file, offset, size)
	_, err = io.Copy(outputFile, reader)
	if err != nil {
		return fmt.Errorf("failed to copy region: %w", err)
	}

	return nil
}

// shrinkExtFilesystem shrinks an ext2/3/4 filesystem to its minimum size.
// Returns the new size in bytes, or the original size if shrinking is not possible.
func shrinkExtFilesystem(imagePath string, originalSize uint64) (uint64, error) {
	// Check if this is an ext filesystem using blkid
	cmd := exec.Command("blkid", "-o", "value", "-s", "TYPE", imagePath)
	output, err := cmd.Output()
	if err != nil {
		return originalSize, fmt.Errorf("failed to detect filesystem type: %w", err)
	}

	fsType := strings.TrimSpace(string(output))
	if fsType != "ext2" && fsType != "ext3" && fsType != "ext4" {
		// Not an ext filesystem, return original size
		return originalSize, nil
	}

	log.WithField("fsType", fsType).WithField("image", imagePath).Info("Shrinking ext filesystem")

	// Run e2fsck to ensure filesystem is clean (required before resize)
	e2fsckCmd := exec.Command("e2fsck", "-f", "-y", imagePath)
	e2fsckCmd.Stdout = os.Stdout
	e2fsckCmd.Stderr = os.Stderr
	err = e2fsckCmd.Run()
	if err != nil {
		// e2fsck returns non-zero even for minor fixes, check if it's fatal
		if exitErr, ok := err.(*exec.ExitError); ok {
			// Exit codes 0, 1, 2 are acceptable (0=no errors, 1=errors corrected, 2=reboot needed but we're on image)
			if exitErr.ExitCode() > 2 {
				return originalSize, fmt.Errorf("e2fsck failed with exit code %d: %w", exitErr.ExitCode(), err)
			}
		} else {
			return originalSize, fmt.Errorf("e2fsck failed: %w", err)
		}
	}

	// Resize to minimum size
	resizeCmd := exec.Command("resize2fs", "-M", imagePath)
	resizeCmd.Stdout = os.Stdout
	resizeCmd.Stderr = os.Stderr
	err = resizeCmd.Run()
	if err != nil {
		return originalSize, fmt.Errorf("resize2fs failed: %w", err)
	}

	// Get the new filesystem size using dumpe2fs
	dumpe2fsCmd := exec.Command("dumpe2fs", "-h", imagePath)
	dumpe2fsOutput, err := dumpe2fsCmd.Output()
	if err != nil {
		return originalSize, fmt.Errorf("dumpe2fs failed: %w", err)
	}

	// Parse the block count and block size
	var blockCount, blockSize uint64
	for _, line := range strings.Split(string(dumpe2fsOutput), "\n") {
		if strings.HasPrefix(line, "Block count:") {
			parts := strings.Fields(line)
			if len(parts) >= 3 {
				fmt.Sscanf(parts[2], "%d", &blockCount)
			}
		} else if strings.HasPrefix(line, "Block size:") {
			parts := strings.Fields(line)
			if len(parts) >= 3 {
				fmt.Sscanf(parts[2], "%d", &blockSize)
			}
		}
	}

	if blockCount == 0 || blockSize == 0 {
		return originalSize, fmt.Errorf("could not parse filesystem size from dumpe2fs")
	}

	newSize := blockCount * blockSize
	log.WithFields(log.Fields{
		"originalSize": originalSize,
		"newSize":      newSize,
		"saved":        originalSize - newSize,
		"savedPercent": float64(originalSize-newSize) / float64(originalSize) * 100,
	}).Info("Filesystem shrunk successfully")

	return newSize, nil
}

// compressFileRegionToFile compresses a portion of a file and writes it to output.
// Returns compressed size and SHA-384 hash.
func compressFileRegionToFile(srcPath string, size int64, outputPath string) (uint64, string, error) {
	srcFile, err := os.Open(srcPath)
	if err != nil {
		return 0, "", fmt.Errorf("failed to open source file: %w", err)
	}
	defer srcFile.Close()

	outputFile, err := os.Create(outputPath)
	if err != nil {
		return 0, "", fmt.Errorf("failed to create output file: %w", err)
	}
	defer outputFile.Close()

	// Create a multi-writer to compute SHA-384 while writing
	sha384Hash := sha512.New384()
	multiWriter := io.MultiWriter(outputFile, sha384Hash)

	// Create zstd encoder
	encoder, err := zstd.NewWriter(multiWriter, zstd.WithEncoderLevel(zstd.SpeedDefault), zstd.WithWindowSize(1<<DefaultZstdWindowLog))
	if err != nil {
		return 0, "", fmt.Errorf("failed to create zstd encoder: %w", err)
	}

	// Read only the specified size
	reader := io.LimitReader(srcFile, size)
	_, err = io.Copy(encoder, reader)
	if err != nil {
		encoder.Close()
		return 0, "", fmt.Errorf("failed to compress file: %w", err)
	}

	if err := encoder.Close(); err != nil {
		return 0, "", fmt.Errorf("failed to close zstd encoder: %w", err)
	}

	// Get the compressed file size
	stat, err := outputFile.Stat()
	if err != nil {
		return 0, "", fmt.Errorf("failed to stat output file: %w", err)
	}

	return uint64(stat.Size()), fmt.Sprintf("%x", sha384Hash.Sum(nil)), nil
}

// compressDataToFile compresses data and writes it to a file.
// Returns the compressed size and SHA-384 hash of the compressed data.
func compressDataToFile(data []byte, outputPath string) (uint64, string, error) {
	outputFile, err := os.Create(outputPath)
	if err != nil {
		return 0, "", fmt.Errorf("failed to create output file: %w", err)
	}
	defer outputFile.Close()

	// Create a multi-writer to compute SHA-384 while writing
	sha384Hash := sha512.New384()
	multiWriter := io.MultiWriter(outputFile, sha384Hash)

	// Create zstd encoder
	encoder, err := zstd.NewWriter(multiWriter, zstd.WithEncoderLevel(zstd.SpeedDefault), zstd.WithWindowSize(1<<DefaultZstdWindowLog))
	if err != nil {
		return 0, "", fmt.Errorf("failed to create zstd encoder: %w", err)
	}

	_, err = encoder.Write(data)
	if err != nil {
		encoder.Close()
		return 0, "", fmt.Errorf("failed to write compressed data: %w", err)
	}

	if err := encoder.Close(); err != nil {
		return 0, "", fmt.Errorf("failed to close zstd encoder: %w", err)
	}

	// Get the file size
	stat, err := outputFile.Stat()
	if err != nil {
		return 0, "", fmt.Errorf("failed to stat output file: %w", err)
	}

	return uint64(stat.Size()), fmt.Sprintf("%x", sha384Hash.Sum(nil)), nil
}

// compressRegionToFile reads a region from a file, compresses it, and writes to output.
// Returns compressed size, uncompressed size, and SHA-384 hash.
func compressRegionToFile(file *os.File, offset int64, size int64, outputPath string) (uint64, uint64, string, error) {
	outputFile, err := os.Create(outputPath)
	if err != nil {
		return 0, 0, "", fmt.Errorf("failed to create output file: %w", err)
	}
	defer outputFile.Close()

	// Create a multi-writer to compute SHA-384 while writing
	sha384Hash := sha512.New384()
	multiWriter := io.MultiWriter(outputFile, sha384Hash)

	// Create zstd encoder
	encoder, err := zstd.NewWriter(multiWriter, zstd.WithEncoderLevel(zstd.SpeedDefault), zstd.WithWindowSize(1<<DefaultZstdWindowLog))
	if err != nil {
		return 0, 0, "", fmt.Errorf("failed to create zstd encoder: %w", err)
	}

	// Read and compress in chunks
	reader := io.NewSectionReader(file, offset, size)
	written, err := io.Copy(encoder, reader)
	if err != nil {
		encoder.Close()
		return 0, 0, "", fmt.Errorf("failed to compress region: %w", err)
	}

	if err := encoder.Close(); err != nil {
		return 0, 0, "", fmt.Errorf("failed to close zstd encoder: %w", err)
	}

	// Get the compressed file size
	stat, err := outputFile.Stat()
	if err != nil {
		return 0, 0, "", fmt.Errorf("failed to stat output file: %w", err)
	}

	return uint64(stat.Size()), uint64(written), fmt.Sprintf("%x", sha384Hash.Sum(nil)), nil
}

// populateFilesystemMetadata decompresses partitions, mounts them, and extracts metadata.
func populateFilesystemMetadata(cosiMeta *metadata.MetadataJson, partInfos []partitionInfo, tmpDir string) error {
	// Track which partition is the root filesystem
	var rootMountPath string
	var espMountPath string

	// First pass: get filesystem info and mount points using blkid
	for i := range partInfos {
		pi := &partInfos[i]

		// Decompress the partition to a temporary file
		decompressedPath := filepath.Join(tmpDir, fmt.Sprintf("partition-%d.raw", pi.partNumber))
		err := decompressFile(pi.imageFile.SourceFile, decompressedPath)
		if err != nil {
			return fmt.Errorf("failed to decompress partition %d: %w", pi.partNumber, err)
		}
		defer os.Remove(decompressedPath)

		// Get filesystem type and UUID using blkid
		fsType, fsUuid, err := getFsData(decompressedPath)
		if err != nil {
			log.WithError(err).WithField("partition", pi.partNumber).Warn("Could not get filesystem data, skipping")
			continue
		}

		pi.fsType = fsType
		pi.fsUuid = fsUuid

		// Determine mount point based on partition type GUID
		pi.mountPoint = determineMountPoint(pi.entry.PartitionTypeGUID)

		log.WithFields(log.Fields{
			"partition":  pi.partNumber,
			"fsType":     pi.fsType,
			"fsUuid":     pi.fsUuid,
			"mountPoint": pi.mountPoint,
		}).Debug("Got filesystem info")
	}

	// Second pass: mount filesystems and extract metadata
	mountTmpDir := filepath.Join(tmpDir, "mounts")
	if err := os.MkdirAll(mountTmpDir, 0755); err != nil {
		return fmt.Errorf("failed to create mounts directory: %w", err)
	}

	for i := range partInfos {
		pi := &partInfos[i]

		if pi.fsType == "" || pi.fsType == "UNKNOWN" {
			continue
		}

		// Create the Image entry
		partType := uuidToPartitionType(pi.entry.PartitionTypeGUID)

		img := metadata.Image{
			Image:      *pi.imageFile,
			MountPoint: pi.mountPoint,
			FsType:     pi.fsType,
			FsUuid:     pi.fsUuid,
			PartType:   partType,
			Verity:     nil,
		}
		cosiMeta.Images = append(cosiMeta.Images, img)

		// Decompress again for mounting (we removed it after blkid)
		decompressedPath := filepath.Join(tmpDir, fmt.Sprintf("partition-%d.raw", pi.partNumber))
		err := decompressFile(pi.imageFile.SourceFile, decompressedPath)
		if err != nil {
			return fmt.Errorf("failed to decompress partition %d for mounting: %w", pi.partNumber, err)
		}

		// Mount the filesystem
		mountPath := filepath.Join(mountTmpDir, fmt.Sprintf("part%d", pi.partNumber))
		if err := os.MkdirAll(mountPath, 0755); err != nil {
			os.Remove(decompressedPath)
			return fmt.Errorf("failed to create mount point: %w", err)
		}

		err = exec.Command("mount", "-o", "loop,ro", decompressedPath, mountPath).Run()
		if err != nil {
			os.Remove(decompressedPath)
			log.WithError(err).WithField("partition", pi.partNumber).Warn("Could not mount partition")
			continue
		}

		// Remember to unmount and cleanup
		defer func(mp, dp string) {
			exec.Command("umount", mp).Run()
			os.Remove(dp)
		}(mountPath, decompressedPath)

		// Track root and ESP mount paths
		if pi.mountPoint == "/" {
			rootMountPath = mountPath
		} else if pi.mountPoint == "/boot/efi" || pi.mountPoint == "/efi" {
			espMountPath = mountPath
		}
	}

	// Extract os-release from root filesystem
	if rootMountPath != "" {
		osRelease, err := extractOsRelease(rootMountPath)
		if err != nil {
			log.WithError(err).Warn("Could not extract os-release")
		} else {
			cosiMeta.OsRelease = osRelease
			cosiMeta.OsArch = detectArchitecture(osRelease)
		}

		// Extract installed packages
		packages, err := extractPackages(rootMountPath)
		if err != nil {
			log.WithError(err).Warn("Could not extract package list")
		} else {
			cosiMeta.OsPackages = packages
		}
	}

	// Verify GRUB is present
	grubFound := false
	if espMountPath != "" {
		grubFound = checkGrubPresence(espMountPath)
	}
	if rootMountPath != "" && !grubFound {
		grubFound = checkGrubPresence(rootMountPath)
	}

	if !grubFound {
		return fmt.Errorf("GRUB bootloader not found; only GRUB is supported")
	}

	return nil
}

// decompressFile decompresses a zstd-compressed file.
func decompressFile(srcPath, dstPath string) error {
	srcFile, err := os.Open(srcPath)
	if err != nil {
		return fmt.Errorf("failed to open source file: %w", err)
	}
	defer srcFile.Close()

	dstFile, err := os.Create(dstPath)
	if err != nil {
		return fmt.Errorf("failed to create destination file: %w", err)
	}
	defer dstFile.Close()

	decoder, err := zstd.NewReader(srcFile, zstd.WithDecoderMaxWindow(1<<30))
	if err != nil {
		return fmt.Errorf("failed to create zstd decoder: %w", err)
	}
	defer decoder.Close()

	_, err = io.Copy(dstFile, decoder)
	if err != nil {
		return fmt.Errorf("failed to decompress: %w", err)
	}

	return nil
}

// getFsData gets filesystem type and UUID using blkid.
func getFsData(imagePath string) (string, string, error) {
	cmd := exec.Command("blkid", "-o", "export", imagePath)
	output, err := cmd.Output()
	if err != nil {
		return "", "", fmt.Errorf("failed to run blkid: %w", err)
	}

	var fsType = "UNKNOWN"
	var fsUuid = "00000000-0000-0000-0000-000000000000"

	for _, line := range strings.Split(string(output), "\n") {
		if after, found := strings.CutPrefix(line, "TYPE="); found {
			fsType = after
		} else if after, found := strings.CutPrefix(line, "UUID="); found {
			fsUuid = after
		}
	}

	return fsType, fsUuid, nil
}

// determineMountPoint returns the mount point based on the partition type GUID.
func determineMountPoint(partTypeGUID uuid.UUID) string {
	guidStr := strings.ToLower(partTypeGUID.String())
	switch guidStr {
	case "c12a7328-f81f-11d2-ba4b-00a0c93ec93b": // EFI System Partition
		return "/boot/efi"
	case "bc13c2ff-59e6-4262-a352-b275fd6f7172": // XBOOTLDR
		return "/boot"
	case "4f68bce3-e8cd-4db1-96e7-fbcaf984b709", // Root x86-64
		"b921b045-1df0-41c3-af44-4c6f280d3fae": // Root ARM64
		return "/"
	case "933ac7e1-2eb4-4f13-b844-0e14e2aef915": // Home
		return "/home"
	case "3b8f8425-20e0-4f3b-907f-1a25a76f98e8": // Srv
		return "/srv"
	case "4d21b016-b534-45c2-a9fb-5c16e091fd2d": // Var
		return "/var"
	case "7ec6f557-3bc5-4aca-b293-16ef5df639d1": // Tmp
		return "/tmp"
	case "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f": // Swap
		return "swap"
	default:
		return "/mnt/unknown"
	}
}

// uuidToPartitionType converts a UUID to a PartitionType.
func uuidToPartitionType(guid uuid.UUID) metadata.PartitionType {
	return metadata.PartitionType(strings.ToLower(guid.String()))
}

// extractOsRelease reads /etc/os-release from the mounted filesystem.
func extractOsRelease(mountPath string) (string, error) {
	osReleasePath := filepath.Join(mountPath, "etc", "os-release")
	data, err := os.ReadFile(osReleasePath)
	if err != nil {
		// Try /usr/lib/os-release as fallback
		osReleasePath = filepath.Join(mountPath, "usr", "lib", "os-release")
		data, err = os.ReadFile(osReleasePath)
		if err != nil {
			return "", fmt.Errorf("could not read os-release: %w", err)
		}
	}
	return string(data), nil
}

// detectArchitecture detects the OS architecture from os-release or system info.
func detectArchitecture(osRelease string) metadata.OsArchitecture {
	// Default to x86_64 if we can't determine
	// In a real implementation, we might want to check the ELF headers of binaries
	return metadata.OsArchitectureX8664
}

// extractPackages extracts the list of installed packages by reading the RPM database directly.
// This does not require the rpm command to be installed on the host system.
func extractPackages(mountPath string) ([]metadata.OsPackage, error) {
	// Try different RPM database file locations
	// go-rpmdb needs the path to a specific database file, not just the directory
	rpmDbPaths := []string{
		// SQLite format (RPM 4.16+)
		filepath.Join(mountPath, "var", "lib", "rpm", "rpmdb.sqlite"),
		filepath.Join(mountPath, "usr", "lib", "sysimage", "rpm", "rpmdb.sqlite"),
		// BerkeleyDB format (older RPM)
		filepath.Join(mountPath, "var", "lib", "rpm", "Packages"),
		filepath.Join(mountPath, "usr", "lib", "sysimage", "rpm", "Packages"),
		// NDB format
		filepath.Join(mountPath, "var", "lib", "rpm", "Packages.db"),
		filepath.Join(mountPath, "usr", "lib", "sysimage", "rpm", "Packages.db"),
	}

	var db *rpmdb.RpmDB
	var err error
	for _, dbPath := range rpmDbPaths {
		if _, statErr := os.Stat(dbPath); os.IsNotExist(statErr) {
			continue
		}
		db, err = rpmdb.Open(dbPath)
		if err == nil {
			log.WithField("path", dbPath).Debug("Opened RPM database")
			break
		}
		log.WithError(err).WithField("path", dbPath).Debug("Failed to open RPM database")
	}

	if db == nil {
		return nil, fmt.Errorf("RPM database not found or could not be opened")
	}
	defer db.Close()

	// List all packages
	pkgList, err := db.ListPackages()
	if err != nil {
		return nil, fmt.Errorf("failed to list packages from RPM database: %w", err)
	}

	var packages []metadata.OsPackage
	for _, pkg := range pkgList {
		packages = append(packages, metadata.OsPackage{
			Name:    pkg.Name,
			Version: pkg.Version,
			Release: pkg.Release,
			Arch:    pkg.Arch,
		})
	}

	return packages, nil
}

// checkGrubPresence checks if GRUB is present in the given mount path.
func checkGrubPresence(mountPath string) bool {
	// Check common GRUB locations
	grubPaths := []string{
		filepath.Join(mountPath, "EFI", "BOOT", "grubx64.efi"),
		filepath.Join(mountPath, "EFI", "BOOT", "BOOTX64.EFI"),
		filepath.Join(mountPath, "boot", "grub2"),
		filepath.Join(mountPath, "boot", "grub"),
		filepath.Join(mountPath, "grub2"),
		filepath.Join(mountPath, "grub"),
	}

	for _, p := range grubPaths {
		if _, err := os.Stat(p); err == nil {
			log.WithField("path", p).Debug("Found GRUB")
			return true
		}
	}

	return false
}
