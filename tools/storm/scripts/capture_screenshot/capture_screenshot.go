package capture_screenshot

import (
	"fmt"

	stormutils "tridenttools/storm/utils"
)

type CaptureScreenshotScriptSet struct {
	CaptureScreenshot CaptureScreenshotScript `cmd:"" help:"Capture VM screenshot."`
}

type CaptureScreenshotScript struct {
	VmName             string `help:"Name of VM." type:"string" default:"virtdeploy-vm-0"`
	ScreenshotFilename string `help:"File name for the screenshot." required:"" type:"string"`
	ArtifactsFolder    string `help:"Folder to save screenshots into." required:"" type:"string"`
}

func (s *CaptureScreenshotScript) Run() error {
	err := stormutils.CaptureScreenshot(s.VmName, s.ArtifactsFolder, s.ScreenshotFilename)
	if err != nil {
		return fmt.Errorf("failed to capture screenshot: %w", err)
	}
	return nil
}
