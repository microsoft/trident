package scripts

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/sirupsen/logrus"
)

type CaptureScreenshotScriptSet struct {
	CaptureScreenshot CaptureScreenshotScript `cmd:"" help:"Capture VM screenshot."`
}

type CaptureScreenshotScript struct {
	VmName             string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
	ScreenshotFilename string `help:"File name for the screenshot." type:"string"`
	ArtifactsFolder    string `help:"Folder to save screenshots into." type:"string"`
}

func (s *CaptureScreenshotScript) Run() error {
	ppmFilename, err := os.CreateTemp("", "ppm")
	if err != nil {
		return fmt.Errorf("failed to create temporary file: %w", err)
	}
	defer os.Remove(ppmFilename.Name())

	err = capturePpmScreenshot(s.VmName, ppmFilename.Name())
	if err != nil {
		return err
	}

	pngPath := filepath.Join(s.ArtifactsFolder, s.ScreenshotFilename)
	if err := convertPpmToPng(ppmFilename.Name(), pngPath); err != nil {
		return err
	}
	return nil
}

func capturePpmScreenshot(vmName string, ppmFilename string) error {
	virshOutput, virshErr := exec.Command("sudo", "virsh", "screenshot", vmName, ppmFilename).CombinedOutput()
	logrus.Tracef("virsh screenshot output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		return virshErr
	}
	return nil
}

func convertPpmToPng(ppmPath string, pngPath string) error {
	virshOutput, virshErr := exec.Command("convert", ppmPath, pngPath).CombinedOutput()
	logrus.Tracef("convert output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		return virshErr
	}
	return nil
}
