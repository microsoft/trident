package helpers

import "storm/pkg/storm"

var TRIDENT_HELPERS = []storm.Helper{
	&CheckSshHelper{},
	&AbUpdateHelper{},
}
