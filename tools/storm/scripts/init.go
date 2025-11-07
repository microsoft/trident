package scripts

import (
	"tridenttools/storm/scripts/acr"
	"tridenttools/storm/scripts/build_extension_images"
	"tridenttools/storm/scripts/capture_screenshot"
	"tridenttools/storm/scripts/display_logs"
)

var TRIDENT_SCRIPTSETS = []any{
	&acr.AcrScriptSet{},
	&build_extension_images.BuildExtensionImagesScriptSet{},
	&capture_screenshot.CaptureScreenshotScriptSet{},
	&display_logs.DisplayLogsScriptSet{},
}
