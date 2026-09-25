package build_extension_images

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/sirupsen/logrus"
)

type BuildExtensionImagesScriptSet struct {
	BuildExtensionImages BuildExtensionImagesScript `cmd:"" help:"Builds sample sysexts and confexts"`
}

type BuildExtensionImagesScript struct {
	NumClones     int  `required:"" help:"Number of sysexts and confexts to build."`
	BuildSysexts  bool `help:"Indicates that test sysext images should be built."`
	BuildConfexts bool `help:"Indicates that test confext images should be built."`
}

func (s *BuildExtensionImagesScript) Run() error {
	if !s.BuildConfexts && !s.BuildSysexts {
		logrus.Warn("Neither --build-sysexts nor --build-confexts is specified. Returning early.")
		return nil
	}

	if s.BuildSysexts {
		err := buildImage("sysext", s.NumClones, ".")
		if err != nil {
			return fmt.Errorf("failed to build sysext images: %w", err)
		}
	}
	if s.BuildConfexts {
		err := buildImage("confext", s.NumClones, ".")
		if err != nil {
			return fmt.Errorf("failed to build confext images: %w", err)
		}
	}

	// Verify the images were created
	rawFiles, err := filepath.Glob("*.raw")
	if err != nil {
		return fmt.Errorf("failed to list raw files: %w", err)
	}
	fmt.Println("Created raw files:")
	for _, file := range rawFiles {
		info, err := os.Stat(file)
		if err != nil {
			return fmt.Errorf("failed to stat file %s: %w", file, err)
		}
		logrus.Infof("Built image: %s %d %s", info.Mode(), info.Size(), file)
	}

	logrus.Infof("Extension images created successfully!")
	return nil
}

// BuildSysextImages builds numClones test sysext images into outputDir.
//
// Exposed so scenarios can provision their own extension images instead of
// depending on a pipeline step having built them and moved them into place,
// which is what makes a local dev-box run possible.
func BuildSysextImages(outputDir string, numClones int) error {
	return buildImage("sysext", numClones, outputDir)
}

// buildImage writes <extType> images into outputDir. The intermediate
// directory tree is staged in a temp dir so it does not litter outputDir.
func buildImage(extType string, numClones int, outputDir string) error {
	stagingRoot, err := os.MkdirTemp("", "storm-extension-build-")
	if err != nil {
		return fmt.Errorf("failed to create staging directory: %w", err)
	}
	defer os.RemoveAll(stagingRoot)

	for i := 1; i <= numClones; i++ {
		extName := fmt.Sprintf("test-%s-%d", extType, i)
		// Create extension-release file
		var dir string
		var fileContent string
		if extType == "sysext" {
			dir = filepath.Join(stagingRoot, fmt.Sprintf("%s-image-%d", extType, i), "usr/lib/extension-release.d")
			err = os.MkdirAll(dir, 0755)
			if err != nil {
				return fmt.Errorf("failed to create sysext directory %s: %w", dir, err)
			}
			fileContent = fmt.Sprintf("ID=_any\nSYSEXT_ID=test-sysext\nSYSEXT_VERSION_ID=%d.0.0\nARCHITECTURE=x86-64\n", i)
		} else {
			dir = filepath.Join(stagingRoot, fmt.Sprintf("%s-image-%d", extType, i), "etc/extension-release.d")
			err = os.MkdirAll(dir, 0755)
			if err != nil {
				return fmt.Errorf("failed to create confext directory %s: %w", dir, err)
			}
			fileContent = fmt.Sprintf("ID=_any\nCONFEXT_ID=test-confext\nCONFEXT_VERSION_ID=%d.0.0\nARCHITECTURE=x86-64\n", i)
		}
		extensionReleaseFile := filepath.Join(dir, fmt.Sprintf("extension-release.%s", extName))
		err = os.WriteFile(extensionReleaseFile, []byte(fileContent), 0644)
		if err != nil {
			return fmt.Errorf("failed to write %s extension-release file %s: %w", extType, extensionReleaseFile, err)
		}

		if extType == "sysext" {
			// Create script that outputs version
			binDir := filepath.Join(stagingRoot, fmt.Sprintf("%s-image-%d", extType, i), "usr/bin")
			err := os.MkdirAll(binDir, 0755)
			if err != nil {
				return fmt.Errorf("failed to create sysext directory %s: %w", binDir, err)
			}
			extensionScriptFile := filepath.Join(binDir, "test-extension.sh")
			err = os.WriteFile(
				extensionScriptFile,
				[]byte(fmt.Sprintf("#!/bin/sh\necho \"%d\"", i)),
				0777,
			)
			if err != nil {
				return fmt.Errorf("failed to write %s extension script file %s: %w", extType, extensionReleaseFile, err)
			}
		}

		// Create DDI files using mksquashfs
		imageDir := filepath.Join(stagingRoot, fmt.Sprintf("%s-image-%d", extType, i))
		rawFile := filepath.Join(outputDir, fmt.Sprintf("%s.raw", extName))
		cmd := exec.Command("mksquashfs", imageDir, rawFile, "-comp", "xz", "-Xbcj", "x86", "-noappend", "-no-xattrs")
		if output, err := cmd.CombinedOutput(); err != nil {
			return fmt.Errorf("failed to create raw file %s: %w: %s", rawFile, err, string(output))
		}
	}
	return nil
}
