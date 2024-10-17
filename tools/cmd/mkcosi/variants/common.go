package variants

import (
	"archive/tar"
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

	"github.com/klauspost/compress/zstd"
	log "github.com/sirupsen/logrus"
)

const CosiFileExtension = ".cosi"

//go:embed os-release
var osRelease string

type ImageVariant interface {
	ExpectedImages() []ExpectedImage
	CommonOpts() CommonOpts
}

type CommonOpts struct {
	Source string `arg:"" help:"Source directory to build COSI from" required:"" type:"path"`
	Output string `arg:"" help:"Output file to write COSI to" required:"" type:"path"`
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

type MetadataJson struct {
	Version   string     `json:"version"`
	Images    []Image    `json:"images"`
	OsRelease string     `json:"osRelease"`
	Kernel    KerlenInfo `json:"kernel"`
}

type Image struct {
	Image      ImageFile     `json:"image"`
	MountPoint string        `json:"mountPoint"`
	FsType     string        `json:"fsType"`
	FsUuid     string        `json:"fsUuid"`
	PartType   PartitionType `json:"partType"`
	Verity     *Verity       `json:"verity"`
}

type Verity struct {
	Image    ImageFile `json:"image"`
	Roothash string    `json:"roothash"`
}

type ImageFile struct {
	Path             string `json:"path"`
	CompressedSize   uint64 `json:"compressedSize"`
	UncompressedSize uint64 `json:"uncompressedSize"`
	Sha384           string `json:"sha384"`
}

type KerlenInfo struct {
	Release string `json:"release"`
	Version string `json:"version"`
}

type PartitionType string

const (
	PartitionTypeEsp                PartitionType = "esp"
	PartitionTypeXbootldr           PartitionType = "xbootldr"
	PartitionTypeSwap               PartitionType = "swap"
	PartitionTypeHome               PartitionType = "home"
	PartitionTypeSrv                PartitionType = "srv"
	PartitionTypeVar                PartitionType = "var"
	PartitionTypeTmp                PartitionType = "tmp"
	PartitionTypeLinuxGeneric       PartitionType = "linux-generic"
	PartitionTypeRoot               PartitionType = "root"
	PartitionTypeRootVerity         PartitionType = "root-verity"
	PartitionTypeRootVeritySig      PartitionType = "root-verity-sig"
	PartitionTypeUsr                PartitionType = "usr"
	PartitionTypeUsrVerity          PartitionType = "usr-verity"
	PartitionTypeUsrVeritySig       PartitionType = "usr-verity-sig"
	PartitionTypeRootAmd64          PartitionType = "root-x86-64"
	PartitionTypeRootAmd64Verity    PartitionType = "root-x86-64-verity"
	PartitionTypeRootAmd64VeritySig PartitionType = "root-x86-64-verity-sig"
	PartitionTypeUsrAmd64           PartitionType = "usr-86-64"
	PartitionTypeUsrAmd64Verity     PartitionType = "usr-x86-64-verity"
	PartitionTypeUsrAmd64VeritySig  PartitionType = "usr-x86-64-verity-sig"
	PartitionTypeRootArm64          PartitionType = "root-arm64"
	PartitionTypeRootArm64Verity    PartitionType = "root-arm64-verity"
	PartitionTypeRootArm64VeritySig PartitionType = "root-arm64-verity-sig"
	PartitionTypeUsrArm64           PartitionType = "usr-arm64"
	PartitionTypeUsrArm64Verity     PartitionType = "usr-arm64-verity"
	PartitionTypeUsrArm64VeritySig  PartitionType = "usr-arm64-verity-sig"
)

type ImageBuildData struct {
	Source   string
	Metadata *Image
}

type ExpectedImage struct {
	Name       string
	PartType   PartitionType
	MountPoint string
}

func buildCosiFile(variant ImageVariant) error {
	expectedImages := variant.ExpectedImages()
	commonOpts := variant.CommonOpts()

	if err := commonOpts.Validate(); err != nil {
		return fmt.Errorf("invalid common options: %w", err)
	}

	metadata := MetadataJson{
		Version:   "1.0",
		OsRelease: osRelease,
		Kernel: KerlenInfo{
			Release: "6.6.47.1-1.azl3",
			Version: "#1 SMP PREEMPT_DYNAMIC Sat Aug 24 02:52:27 UTC 2024",
		},
		Images: make([]Image, len(expectedImages)),
	}

	if len(expectedImages) == 0 {
		return errors.New("no images to build")
	}

	imageData := make([]ImageBuildData, len(expectedImages))
	for i, image := range expectedImages {
		metadata := &metadata.Images[i]
		imageData[i] = ImageBuildData{
			Source:   path.Join(commonOpts.Source, image.Name),
			Metadata: metadata,
		}

		metadata.Image.Path = path.Join("images", image.Name)
		metadata.PartType = image.PartType
		metadata.MountPoint = image.MountPoint
	}

	// Find all images in the source directory
	for _, data := range imageData {
		log.WithField("image", data.Source).Info("Processing image...")
		err := data.populateMetadata()
		if err != nil {
			return fmt.Errorf("failed to populate metadata for %s: %w", data.Source, err)
		}
		log.WithField("image", data.Source).Info("Populated metadata for image.")
	}

	// Marshal the metadata to json
	metadataJson, err := json.MarshalIndent(metadata, "", "  ")
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

	// Create tar writer
	tw := tar.NewWriter(cosiFile)
	defer tw.Close()

	tw.WriteHeader(&tar.Header{
		Typeflag: tar.TypeReg,
		Name:     "metadata.json",
		Size:     int64(len(metadataJson)),
		Mode:     0o400,
		Format:   tar.FormatPAX,
	})
	tw.Write(metadataJson)

	for _, data := range imageData {
		log.WithField("image", data.Source).Info("Adding image to COSI...")
		if err := data.addToCosi(tw); err != nil {
			return fmt.Errorf("failed to add %s to COSI: %w", data.Source, err)
		}
	}

	log.WithField("output", commonOpts.Output).Info("Finished building COSI.")

	return nil
}

func (data *ImageBuildData) addToCosi(tw *tar.Writer) error {
	imageFile, err := os.Open(data.Source)
	if err != nil {
		return fmt.Errorf("failed to open image file: %w", err)
	}
	defer imageFile.Close()

	err = tw.WriteHeader(&tar.Header{
		Typeflag: tar.TypeReg,
		Name:     data.Metadata.Image.Path,
		Size:     int64(data.Metadata.Image.CompressedSize),
		Mode:     0o400,
		Format:   tar.FormatPAX,
	})
	if err != nil {
		return fmt.Errorf("failed to write tar header: %w", err)
	}

	_, err = io.Copy(tw, imageFile)
	if err != nil {
		return fmt.Errorf("failed to write image to COSI: %w", err)
	}

	return nil
}

func sha384sum(path string) (string, error) {
	sha384 := sha512.New384()
	file, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer file.Close()

	if _, err := io.Copy(sha384, file); err != nil {
		return "", err
	}
	return fmt.Sprintf("%x", sha384.Sum(nil)), nil
}

func decompressImage(source string) (*os.File, error) {
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

	var fsType, fsUuid string = "NOT_FOUND", "NOT_FOUND"

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

func (data *ImageBuildData) populateMetadata() error {
	stat, err := os.Stat(data.Source)
	if err != nil {
		return fmt.Errorf("filed to stat %s: %w", data.Source, err)
	}
	if stat.IsDir() {
		return fmt.Errorf("%s is a directory", data.Source)
	}
	data.Metadata.Image.CompressedSize = uint64(stat.Size())

	// Calculate the sha384 of the image
	sha384, err := sha384sum(data.Source)
	if err != nil {
		return fmt.Errorf("failed to calculate sha384 of %s: %w", data.Source, err)
	}
	data.Metadata.Image.Sha384 = sha384

	// Decompress the image
	tmpFile, err := decompressImage(data.Source)
	if err != nil {
		return fmt.Errorf("failed to decompress %s: %w", data.Source, err)
	}
	defer tmpFile.Close()

	stat, err = tmpFile.Stat()
	if err != nil {
		return fmt.Errorf("failed to stat decompressed image: %w", err)
	}

	data.Metadata.Image.UncompressedSize = uint64(stat.Size())
	fsType, fsUuid, err := getFsData(tmpFile.Name())
	if err != nil {
		return fmt.Errorf("failed to get filesystem data for %s: %w", data.Source, err)
	}

	data.Metadata.FsType = fsType
	data.Metadata.FsUuid = fsUuid

	return nil
}
