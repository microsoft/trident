package scripts

import (
	"tridenttools/storm/scripts/acr"
	"tridenttools/storm/scripts/build_extension_images"
	"tridenttools/storm/scripts/capture_screenshot"
	"tridenttools/storm/scripts/process_metrics"
	"tridenttools/storm/scripts/storm_acr"
)

var TRIDENT_SCRIPTSETS = []any{
	&acr.AcrScriptSet{},
	&build_extension_images.BuildExtensionImagesScriptSet{},
	&capture_screenshot.CaptureScreenshotScriptSet{},
	&process_metrics.ProcessMetricsScriptSet{},
	&storm_acr.StormAcrScriptSet{},
}
