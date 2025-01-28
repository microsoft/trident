package builder

import (
	"argus_toolkit/cmd/mkcosi/metadata"
	"argus_toolkit/pkg/ref"
	"crypto/sha512"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path"
	"strings"

	"github.com/google/uuid"
	"github.com/klauspost/compress/zstd"
	log "github.com/sirupsen/logrus"
)

const CosiFileExtension = ".cosi"

type ImageVariant interface {
	ExpectedImages() []ExpectedImage
	CommonOpts() CommonOpts
	IsVerity() bool
}

type CommonOpts struct {
	Source          string `arg:"" help:"Source directory to build COSI from." required:"" type:"path"`
	Output          string `arg:"" help:"Output file to write COSI to." required:"" type:"path"`
	SourceExtension string `name:"extension" short:"e" help:"Source file extension." default:"rawzst"`
}

func (opts CommonOpts) Validate() error {
	if path.Ext(opts.Output) != CosiFileExtension {
		return fmt.Errorf("output file must have the extension %s", CosiFileExtension)
	}

	stat, err := os.Stat(opts.Source)
	if err != nil {
		return fmt.Errorf("failed to stat source directory: %w", err)
	}

	if !stat.IsDir() {
		return fmt.Errorf("source must be a directory")
	}

	return nil
}

type ImageBuildData struct {
	KnownInfo ExpectedImage
	Metadata  *metadata.Image
}

type ExpectedImage struct {
	Name                string
	PartType            metadata.PartitionType
	MountPoint          string
	OsReleasePath       *string
	GrubCfgPath         *string
	ContainsRpmDatabase bool
	VerityImageName     *string
}

func (ex ExpectedImage) ShouldMount() bool {
	return ex.OsReleasePath != nil || ex.GrubCfgPath != nil || ex.ContainsRpmDatabase
}

type ExtractedImageData struct {
	OsRelease *string
	GrubCfg   *string
}

func buildCosiFile(variant ImageVariant) error {
	expectedImages := variant.ExpectedImages()
	commonOpts := variant.CommonOpts()

	if err := commonOpts.Validate(); err != nil {
		return fmt.Errorf("invalid common options: %w", err)
	}

	cosiMetadata := metadata.MetadataJson{
		Version: "1.0",
		OsArch:  "x86_64",
		Id:      uuid.New().String(),
		Images:  make([]metadata.Image, len(expectedImages)),
	}

	if len(expectedImages) == 0 {
		return errors.New("no images to build")
	}

	// Pointer to wherever we need to write the roothash to
	var roothash *string = nil

	// Create an interim metadata struct to combine the known data with the
	// metadata we need to populate.
	imageData := make([]ImageBuildData, len(expectedImages))
	for i, image := range expectedImages {
		// Ge the full name of the image
		full_image_name := fmt.Sprintf("%s.%s", image.Name, commonOpts.SourceExtension)
		// Get a reference to the imgMetadata for this index
		imgMetadata := &cosiMetadata.Images[i]
		// Populate the image build data for this index
		imageData[i] = ImageBuildData{
			Metadata:  imgMetadata,
			KnownInfo: image,
		}

		// Populate the image source
		imgMetadata.Image.SourceFile = path.Join(commonOpts.Source, full_image_name)
		log.WithField("path", imgMetadata.Image.SourceFile).Debug("Adding expected image to list.")
		// Populate the in-COSI file path
		imgMetadata.Image.Path = path.Join("images", full_image_name)
		// Populate the partition type
		imgMetadata.PartType = image.PartType
		// Populate the mount point
		imgMetadata.MountPoint = image.MountPoint
		// Populate verity data if needed
		if variant.IsVerity() && image.VerityImageName != nil {
			full_verity_image_name := fmt.Sprintf("%s.%s", *image.VerityImageName, commonOpts.SourceExtension)
			imgMetadata.Verity = &metadata.Verity{
				Image: metadata.ImageFile{
					Path:       path.Join("images", full_verity_image_name),
					SourceFile: path.Join(commonOpts.Source, full_verity_image_name),
				},
			}

			log.WithField("path", imgMetadata.Verity.Image.SourceFile).Debug("Adding expected image to list.")

			// Set the pointer to the roothash
			roothash = &imgMetadata.Verity.Roothash
		}
	}

	// If we're building a verity image, one image should have defined a verity
	// hash image, and we should have a pointer to write the roothash to
	if variant.IsVerity() && roothash == nil {
		return errors.New("OS Image is declared as verity but none of the filesystem images has verity metadata")
	}

	// Find all images in the source directory
	for _, data := range imageData {
		if data.Metadata.Image.SourceFile == "" {
			return errors.New("source file not set")
		}
		source := data.Metadata.Image.SourceFile

		log.WithField("image", source).Info("Processing image...")
		extracted, err := data.populateMetadata()
		if err != nil {
			return fmt.Errorf("failed to populate metadata for %s: %w", source, err)
		}
		log.WithField("image", source).Info("Populated metadata for image.")

		if extracted.OsRelease != nil {
			log.Debugf("Populated os-release metadata from image %s", source)
			cosiMetadata.OsRelease = *extracted.OsRelease
		}

		if variant.IsVerity() && extracted.GrubCfg != nil {
			log.WithField("image", source).Info("Found verity grub.cfg, extracting roothash...")
			extractedRoothash, err := extractRoothash(*extracted.GrubCfg)
			if err != nil {
				return fmt.Errorf("failed to extract roothash from %s: %w", source, err)
			}

			// Write the roothash to the pointer
			*roothash = extractedRoothash
		}
	}

	if variant.IsVerity() && *roothash == "" {
		return errors.New("no image provided grub.cfg to extract the roothash from")
	}

	// Marshal the metadata to json
	metadataJson, err := json.MarshalIndent(cosiMetadata, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal metadata: %w", err)
	}

	log.Info("Finished metadata generation:\n", string(metadataJson))

	// Create COSI file
	cosiFile, err := os.Create(commonOpts.Output)
	if err != nil {
		return fmt.Errorf("failed to create COSI file: %w", err)
	}
	defer cosiFile.Close()

	err = BuildCosi(cosiFile, &cosiMetadata)
	if err != nil {
		return fmt.Errorf("failed to build COSI file: %w", err)
	}

	log.WithField("output", commonOpts.Output).Info("Finished building COSI.")

	return nil
}

func DecompressImage(source string) (*os.File, error) {
	src, err := os.Open(source)
	if err != nil {
		return nil, fmt.Errorf("failed to open %s: %w", source, err)
	}
	defer src.Close()

	tmpFile, err := os.CreateTemp("", "mkcosi")
	if err != nil {
		return nil, fmt.Errorf("failed to create temporary file: %w", err)
	}

	zr, err := zstd.NewReader(src)
	if err != nil {
		tmpFile.Close()
		return nil, fmt.Errorf("failed to create zstd reader: %w", err)
	}

	if _, err := io.Copy(tmpFile, zr); err != nil {
		tmpFile.Close()
		return nil, fmt.Errorf("failed to decompress %s: %w", source, err)
	}

	zr.Close()

	return tmpFile, nil
}

func getFsData(imagePath string) (string, string, error) {
	cmd := exec.Command("blkid", "-o", "export", imagePath)
	output, err := cmd.Output()
	if err != nil {
		return "", "", fmt.Errorf("failed to run blkid: %w", err)
	}

	// Default to unknown filesystem type
	var fsType = "UNKNOWN"
	// Default to zero UUID
	var fsUuid = "00000000-0000-0000-0000-000000000000"

	var outputLines = strings.Split(string(output), "\n")

	for _, line := range outputLines {
		if after, found := strings.CutPrefix(line, "TYPE="); found {
			fsType = after
		} else if after, found := strings.CutPrefix(line, "UUID="); found {
			fsUuid = after
		}
	}

	return fsType, fsUuid, nil
}

func (data *ImageBuildData) populateMetadata() (*ExtractedImageData, error) {
	if data.Metadata.Image.SourceFile == "" {
		return nil, fmt.Errorf("source file not set")
	}
	source := data.Metadata.Image.SourceFile

	stat, err := os.Stat(source)
	if err != nil {
		return nil, fmt.Errorf("filed to stat %s: %w", source, err)
	}
	if stat.IsDir() {
		return nil, fmt.Errorf("%s is a directory", source)
	}
	data.Metadata.Image.CompressedSize = uint64(stat.Size())

	// Calculate the sha384 of the image
	sha384, err := Sha384SumFile(source)
	if err != nil {
		return nil, fmt.Errorf("failed to calculate sha384 of %s: %w", source, err)
	}
	data.Metadata.Image.Sha384 = sha384

	// Decompress the image
	tmpFile, err := DecompressImage(source)
	if err != nil {
		return nil, fmt.Errorf("failed to decompress %s: %w", source, err)
	}
	defer os.Remove(tmpFile.Name())
	defer tmpFile.Close()

	stat, err = tmpFile.Stat()
	if err != nil {
		return nil, fmt.Errorf("failed to stat decompressed image: %w", err)
	}

	data.Metadata.Image.UncompressedSize = uint64(stat.Size())
	data.Metadata.FsType, data.Metadata.FsUuid, err = getFsData(tmpFile.Name())
	if err != nil {
		return nil, fmt.Errorf("failed to get filesystem data for %s: %w", source, err)
	}

	temp_mount_path, err := os.MkdirTemp("", "mkcosi")
	if err != nil {
		return nil, fmt.Errorf("failed to create temporary mount path: %w", err)
	}

	err = populateVerityMetadata(data.Metadata.Verity)
	if err != nil {
		return nil, fmt.Errorf("failed to populate verity metadata: %w", err)
	}

	var extractedData ExtractedImageData

	// If this image doesn't need to be mounted, we're done
	if !data.KnownInfo.ShouldMount() {
		return &extractedData, nil
	}

	mount, err := NewLoopDevMount(tmpFile.Name(), temp_mount_path)
	if err != nil {
		return nil, fmt.Errorf("failed to mount %s: %w", tmpFile.Name(), err)
	}
	defer mount.Close()

	// If this image contains os-release, extract it...
	if data.KnownInfo.OsReleasePath != nil {
		osReleasePath := path.Join(mount.Path(), *data.KnownInfo.OsReleasePath)
		osReleaseData, err := os.ReadFile(osReleasePath)
		if err != nil {
			return nil, fmt.Errorf("failed to read %s: %w", osReleasePath, err)
		}
		extractedData.OsRelease = ref.Of(string(osReleaseData))
	}

	// If this image contains grub.cfg, extract it...
	if data.KnownInfo.GrubCfgPath != nil {
		grubCfgPath := path.Join(mount.Path(), *data.KnownInfo.GrubCfgPath)
		grubCfgData, err := os.ReadFile(grubCfgPath)
		if err != nil {
			return nil, fmt.Errorf("failed to read %s: %w", grubCfgPath, err)
		}
		extractedData.GrubCfg = ref.Of(string(grubCfgData))
	}

	return &extractedData, nil
}

func populateVerityMetadata(verity *metadata.Verity) error {
	if verity == nil {
		return nil
	}

	if verity.Image.SourceFile == "" {
		return fmt.Errorf("verity source file not set")
	}

	source := verity.Image.SourceFile

	verityFile := &verity.Image

	verityStat, err := os.Stat(source)
	if err != nil {
		return fmt.Errorf("failed to stat verity source: %w", err)
	}

	verityFile.CompressedSize = uint64(verityStat.Size())

	veritySha384, err := Sha384SumFile(source)
	if err != nil {
		return fmt.Errorf("failed to calculate sha384 of verity source: %w", err)
	}

	verityFile.Sha384 = veritySha384

	verityTmpFile, err := DecompressImage(source)
	if err != nil {
		return fmt.Errorf("failed to decompress verity source: %w", err)
	}
	defer verityTmpFile.Close()

	verityStat, err = verityTmpFile.Stat()
	if err != nil {
		return fmt.Errorf("failed to stat decompressed verity source: %w", err)
	}

	verityFile.UncompressedSize = uint64(verityStat.Size())

	return nil
}

func Sha384SumFile(path string) (string, error) {
	file, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer file.Close()

	return Sha384SumReader(file)
}

func Sha384SumReader(reader io.Reader) (string, error) {
	sha384 := sha512.New384()
	if _, err := io.Copy(sha384, reader); err != nil {
		return "", err
	}
	return fmt.Sprintf("%x", sha384.Sum(nil)), nil
}
