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

	"github.com/google/uuid"
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
	OsArch    string     `json:"osArch"`
	Images    []Image    `json:"images"`
	OsRelease string     `json:"osRelease"`
	Kernel    KerlenInfo `json:"kernel"`
	Id        string     `json:"id"`
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
	PartitionTypeEsp                PartitionType = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
	PartitionTypeXbootldr           PartitionType = "bc13c2ff-59e6-4262-a352-b275fd6f7172"
	PartitionTypeSwap               PartitionType = "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f"
	PartitionTypeHome               PartitionType = "933ac7e1-2eb4-4f13-b844-0e14e2aef915"
	PartitionTypeSrv                PartitionType = "3b8f8425-20e0-4f3b-907f-1a25a76f98e8"
	PartitionTypeVar                PartitionType = "4d21b016-b534-45c2-a9fb-5c16e091fd2d"
	PartitionTypeTmp                PartitionType = "7ec6f557-3bc5-4aca-b293-16ef5df639d1"
	PartitionTypeLinuxGeneric       PartitionType = "0fc63daf-8483-4772-8e79-3d69d8477de4"
	PartitionTypeRootAmd64          PartitionType = "4f68bce3-e8cd-4db1-96e7-fbcaf984b709"
	PartitionTypeRootAmd64Verity    PartitionType = "2c7357ed-ebd2-46d9-aec1-23d437ec2bf5"
	PartitionTypeRootAmd64VeritySig PartitionType = "41092b05-9fc8-4523-994f-2def0408b176"
	PartitionTypeUsrAmd64           PartitionType = "8484680c-9521-48c6-9c11-b0720656f69e"
	PartitionTypeUsrAmd64Verity     PartitionType = "77ff5f63-e7b6-4633-acf4-1565b864c0e6"
	PartitionTypeUsrAmd64VeritySig  PartitionType = "e7bb33fb-06cf-4e81-8273-e543b413e2e2"
	PartitionTypeRootArm64          PartitionType = "b921b045-1df0-41c3-af44-4c6f280d3fae"
	PartitionTypeRootArm64Verity    PartitionType = "df3300ce-d69f-4c92-978c-9bfb0f38d820"
	PartitionTypeRootArm64VeritySig PartitionType = "6db69de6-29f4-4758-a7a5-962190f00ce3"
	PartitionTypeUsrArm64           PartitionType = "b0e01050-ee5f-4390-949a-9101b17104e9"
	PartitionTypeUsrArm64Verity     PartitionType = "6e11a4e7-fbca-4ded-b9e9-e1a512bb664e"
	PartitionTypeUsrArm64VeritySig  PartitionType = "c23ce4ff-44bd-4b00-b2d4-b41b3419e02a"

	PartitionTypeRoot          PartitionType = PartitionTypeRootAmd64
	PartitionTypeRootVerity    PartitionType = PartitionTypeRootAmd64Verity
	PartitionTypeRootVeritySig PartitionType = PartitionTypeRootAmd64VeritySig
	PartitionTypeUsr           PartitionType = PartitionTypeUsrAmd64
	PartitionTypeUsrVerity     PartitionType = PartitionTypeUsrAmd64Verity
	PartitionTypeUsrVeritySig  PartitionType = PartitionTypeUsrAmd64VeritySig
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
		OsArch:    "x86_64",
		OsRelease: osRelease,
		Id:        uuid.New().String(),
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
