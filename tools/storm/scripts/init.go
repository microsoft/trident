package scripts

import (
	"tridenttools/storm/scripts/acr"
	"tridenttools/storm/scripts/build_extension_images"
)

var TRIDENT_SCRIPTSETS = []any{
	&acr.AcrScriptSet{},
	&build_extension_images.BuildExtensionImagesScriptSet{},
}
