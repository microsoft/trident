package helpers

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type CaptureScreenshotHelper struct {
	args struct {
		VmName             string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		ScreenshotFilename string `help:"File name for the screenshot." type:"string"`
		ArtifactsFolder    string `help:"Folder to save screenshots into." type:"string"`
	}
}

func (h CaptureScreenshotHelper) Name() string {
	return "capture-screenshot"
}

func (h *CaptureScreenshotHelper) Args() any {
	return &h.args
}

func (h *CaptureScreenshotHelper) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("capture-screenshot", h.captureScreenshot)
	return nil
}

func (h *CaptureScreenshotHelper) capturePpmScreenshot(vmName string, ppmFilename string) error {
	virshOutput, virshErr := exec.Command("sudo", "virsh", "screenshot", vmName, ppmFilename).CombinedOutput()
	logrus.Tracef("virsh screenshot output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		return virshErr
	}
	return nil
}

func (h *CaptureScreenshotHelper) convertPpmToPng(ppmPath string, pngPath string) error {
	virshOutput, virshErr := exec.Command("convert", ppmPath, pngPath).CombinedOutput()
	logrus.Tracef("convert output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		return virshErr
	}
	return nil
}

func (h *CaptureScreenshotHelper) captureScreenshot(tc storm.TestCase) error {
	ppmFilename, err := os.CreateTemp("", "ppm")
	if err != nil {
		return fmt.Errorf("failed to create temporary file: %w", err)
	}
	defer os.Remove(ppmFilename.Name())

	err = h.capturePpmScreenshot(h.args.VmName, ppmFilename.Name())
	if err != nil {
		return err
	}

	pngPath := filepath.Join(h.args.ArtifactsFolder, h.args.ScreenshotFilename)
	if err := h.convertPpmToPng(ppmFilename.Name(), pngPath); err != nil {
		return err
	}
	return nil
}
