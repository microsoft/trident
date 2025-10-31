package helpers

import "github.com/microsoft/storm"

var TRIDENT_HELPERS = []storm.Helper{
	&CheckSshHelper{},
	&AbUpdateHelper{},
	&PrepareImages{},
	&BootMetricsHelper{},
	&CheckSelinuxHelper{},
	&BuildExtensionImagesHelper{},
	&RebuildRaidHelper{},
}
