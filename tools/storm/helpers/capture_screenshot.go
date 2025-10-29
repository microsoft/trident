package helpers

import (
	"os/exec"
	"path/filepath"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"
)

type CaptureScreenshotHelper struct {
	args struct {
		VmName             string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
		ScreenshotFilename string `help:"File name for the screenshot." type:"string" default:""`
		ArtifactsFolder    string `help:"Folder to copy log files into." type:"string" default:""`
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

func (h *CaptureScreenshotHelper) capturePpmScreenshot(vmName string) (string, error) {
	ppmFilename := "/tmp/screenshot.ppm"
	virshOutput, virshErr := exec.Command("sudo", "virsh", "screenshot", vmName, ppmFilename).CombinedOutput()
	logrus.Tracef("virsh screenshot output: %s\n%v", string(virshOutput), virshErr)
	if virshErr != nil {
		return "", virshErr
	}
	return ppmFilename, nil
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
	ppmPath, err := h.capturePpmScreenshot(h.args.VmName)
	if err != nil {
		return err
	}

	pngPath := filepath.Join(h.args.ArtifactsFolder, h.args.ScreenshotFilename)
	if err := h.convertPpmToPng(ppmPath, pngPath); err != nil {
		return err
	}
	return nil
}
