package helpers

import "storm"

var TRIDENT_HELPERS = []storm.Helper{
	&CheckSshHelper{},
	&AbUpdateHelper{},
	&PrepareImages{},
	&BootMetricsHelper{},
	&CheckSelinuxHelper{},
}
