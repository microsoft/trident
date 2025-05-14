package helpers

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"storm/pkg/storm"

	"github.com/sirupsen/logrus"
)

const (
	COSI_EXTENSION              = "cosi"
	OUTPUT_REGULAR_IMAGE_NAME   = "regular"
	OUTPUT_VERITY_IMAGE_NAME    = "verity"
	OUTPUT_USRVERITY_IMAGE_NAME = "usrverity"
)

type PrepareImages struct {
	args struct {
		RegularTestImageDir   string `arg:"" help:"Directory containing the regular test images" type:"path"`
		VerityTestImageDir    string `arg:"" help:"Directory containing the verity test images" type:"path"`
		UsrVerityTestImageDir string `arg:"" help:"Directory containing the verity test images" type:"path"`
		RegularImageName      string `arg:"" help:"Name of the regular test image"`
		VerityImageName       string `arg:"" help:"Name of the verity test image"`
		UsrVerityImageName    string `arg:"" help:"Name of the verity test image"`
		OutputDir             string `arg:"" help:"Directory in which to place the prepared images" type:"path"`
		Versions              uint   `short:"v" help:"Number of versions to create of each image type" default:"1"`
	}
}

func (h PrepareImages) Name() string {
	return "prepare-images"
}

func (h *PrepareImages) Args() any {
	return &h.args
}

func (h *PrepareImages) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("copy-regular", h.copyRegularImages)
	r.RegisterTestCase("copy-verity", h.copyVerityImages)
	r.RegisterTestCase("copy-usrverity", h.copyUsrVerityImages)
	return nil
}

func (h *PrepareImages) copyRegularImages(tc storm.TestCase) error {
	// Skip test if the path doesn't exist
	if _, err := os.Stat(h.args.RegularTestImageDir); os.IsNotExist(err) {
		tc.Skip(fmt.Sprintf("Directory %s does not exist", h.args.RegularTestImageDir))
	}

	return copyImages(
		tc.Logger(),
		h.args.RegularTestImageDir,
		h.args.OutputDir,
		h.args.RegularImageName,
		COSI_EXTENSION,
		OUTPUT_REGULAR_IMAGE_NAME,
		h.args.Versions,
	)
}

func (h *PrepareImages) copyVerityImages(tc storm.TestCase) error {
	// Skip test if the path doesn't exist
	if _, err := os.Stat(h.args.VerityTestImageDir); os.IsNotExist(err) {
		tc.Skip(fmt.Sprintf("Directory %s does not exist", h.args.VerityTestImageDir))
	}

	return copyImages(
		tc.Logger(),
		h.args.VerityTestImageDir,
		h.args.OutputDir,
		h.args.VerityImageName,
		COSI_EXTENSION,
		OUTPUT_VERITY_IMAGE_NAME,
		h.args.Versions,
	)
}

func (h *PrepareImages) copyUsrVerityImages(tc storm.TestCase) error {
	// Skip test if the path doesn't exist
	if _, err := os.Stat(h.args.UsrVerityTestImageDir); os.IsNotExist(err) {
		tc.Skip(fmt.Sprintf("Directory %s does not exist", h.args.UsrVerityTestImageDir))
	}

	return copyImages(
		tc.Logger(),
		h.args.UsrVerityTestImageDir,
		h.args.OutputDir,
		h.args.UsrVerityImageName,
		COSI_EXTENSION,
		OUTPUT_USRVERITY_IMAGE_NAME,
		h.args.Versions,
	)
}

func copyImages(log *logrus.Logger, srcDir, destDir string, imageName string, ext string, outputFilename string, versions uint) error {
	srcDir, err := filepath.Abs(srcDir)
	if err != nil {
		return fmt.Errorf("failed to get absolute path of source directory %s: %v", srcDir, err)
	}
	destDir, err = filepath.Abs(destDir)
	if err != nil {
		return fmt.Errorf("failed to get absolute path of destination directory %s: %v", destDir, err)
	}

	glob := fmt.Sprintf("%s/%s*.%s", srcDir, imageName, ext)
	files, err := filepath.Glob(glob)
	if err != nil {
		return fmt.Errorf("failed to list files in directory %s: %v", srcDir, err)
	}

	if len(files) == 0 {
		return fmt.Errorf("no '%s' files found in directory %s", glob, srcDir)
	}

	log.Infof("Found %d files in %s matching glob %s", len(files), srcDir, glob)

	singleFilePattern := fmt.Sprintf("%s.%s", imageName, ext)
	multipleFilePattern := fmt.Sprintf(`%s_(\d+).%s`, regexp.QuoteMeta(imageName), regexp.QuoteMeta(ext))

	if len(files) == 1 && filepath.Base(files[0]) != singleFilePattern {
		// Single file, must be names exactly as the image name + extension
		return fmt.Errorf("file '%s' does not match the expected pattern '%s'", filepath.Base(files[0]), singleFilePattern)
	} else if len(files) > 1 {
		compiled, err := regexp.Compile(multipleFilePattern)
		if err != nil {
			return fmt.Errorf("failed to compile regex %s: %v", multipleFilePattern, err)
		}

		// Multiple files, must match the pattern imageName_0.ext, imageName_1.ext, etc.
		for _, file := range files {
			if !compiled.MatchString(filepath.Base(file)) {
				return fmt.Errorf("file %s does not match the expected pattern %s", file, multipleFilePattern)
			}
		}
	}

	// Create output directory if it doesn't exist
	if _, err := os.Stat(destDir); os.IsNotExist(err) {
		log.Debugf("Creating directory %s", destDir)
		err := os.MkdirAll(destDir, 0755)
		if err != nil {
			return fmt.Errorf("failed to create directory %s: %v", destDir, err)
		}
	}

	outputFiles := make([]string, 0)

	for i, file := range files {
		var newFileName string
		if i == 0 {
			newFileName = fmt.Sprintf("%s.%s", outputFilename, ext)
		} else {
			// Add 1 because we expect the first update to consume v2
			newFileName = fmt.Sprintf("%s_v%d.%s", outputFilename, i+1, ext)
		}

		log.Infof("Moving file '%s' to '%s'", file, newFileName)

		newFilePath := filepath.Join(destDir, newFileName)
		err := os.Rename(file, newFilePath)
		if err != nil {
			return fmt.Errorf("failed to rename file %s to %s: %v", file, newFilePath, err)
		}

		outputFiles = append(outputFiles, newFilePath)
	}

	for v := len(outputFiles); v < int(versions); v++ {
		// Add 1 because we expect the first update to consume v2
		newFileName := fmt.Sprintf("%s_v%d.%s", outputFilename, v+1, ext)
		baseFile := outputFiles[v%len(outputFiles)]
		// Create a hard link to the base file
		newFilePath := filepath.Join(destDir, newFileName)
		log.Infof("Linking file '%s' to '%s'", baseFile, newFilePath)
		err := os.Link(baseFile, newFilePath)
		if err != nil {
			return fmt.Errorf("failed to link file %s to %s: %v", baseFile, newFilePath, err)
		}
	}

	return nil
}
