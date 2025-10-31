package scripts

import "tridenttools/storm/scripts/acr"

var TRIDENT_SCRIPTSETS = []any{
	&acr.AcrScriptSet{},
	&BuildExtensionImagesScriptSet{},
}
