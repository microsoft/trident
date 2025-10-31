package scripts

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/sirupsen/logrus"
)

type BuildExtensionImagesScriptSet struct {
	// This field represents the "build-extension-images" subcommand.
	BuildExtensionImages BuildExtensionImagesScript `cmd:"" help:"Builds sample sysexts and confexts"`
}

type BuildExtensionImagesScript struct {
	NumClones     int  `required:"" help:"Number of sysexts and confexts to build." type:"int"`
	BuildSysexts  bool `help:"Indicates that test sysext images should be built." type:"bool"`
	BuildConfexts bool `help:"Indicates that test confext images should be built." type:"bool"`
}

func (s *BuildExtensionImagesScript) Run() error {
	if s.BuildSysexts {
		err := buildImage("sysext", s.NumClones)
		if err != nil {
			return fmt.Errorf("failed to build sysext images: %w", err)
		}
	}
	if s.BuildConfexts {
		err := buildImage("confext", s.NumClones)
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

func buildImage(extType string, numClones int) error {
	for i := 1; i <= numClones; i++ {
		// Create extension-release file
		var dir string
		var fileContent string
		var err error
		if extType == "sysext" {
			dir = fmt.Sprintf("%s-image-%d/usr/lib/extension-release.d", extType, i)
			err = os.MkdirAll(dir, 0755)
			if err != nil {
				return fmt.Errorf("failed to create sysext directory %s: %w", dir, err)
			}
			fileContent = fmt.Sprintf("ID=_any\nSYSEXT_ID=test-sysext\nSYSEXT_VERSION_ID=%d.0.0\nARCHITECTURE=x86-64\n", i)
		} else {
			dir = fmt.Sprintf("%s-image-%d/etc/extension-release.d", extType, i)
			err = os.MkdirAll(dir, 0755)
			if err != nil {
				return fmt.Errorf("failed to create confext directory %s: %w", dir, err)
			}
			fileContent = fmt.Sprintf("ID=_any\nCONFEXT_ID=test-confext\nCONFEXT_VERSION_ID=%d.0.0\nARCHITECTURE=x86-64\n", i)
		}
		extensionReleaseFile := filepath.Join(dir, fmt.Sprintf("extension-release.test-%s", extType))
		err = os.WriteFile(extensionReleaseFile, []byte(fileContent), 0644)
		if err != nil {
			return fmt.Errorf("failed to write %s extension-release file %s: %w", extType, extensionReleaseFile, err)
		}

		// Create DDI files using mksquashfs
		imageDir := fmt.Sprintf("%s-image-%d", extType, i)
		rawFile := fmt.Sprintf("test-%s-%d.raw", extType, i)
		cmd := exec.Command("mksquashfs", imageDir, rawFile, "-comp", "xz", "-Xbcj", "x86", "-noappend", "-no-xattrs")
		err = cmd.Run()
		if err != nil {
			return fmt.Errorf("failed to create raw file %s: %w", rawFile, err)
		}
	}
	return nil
}
