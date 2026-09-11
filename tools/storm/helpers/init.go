package helpers

import "github.com/microsoft/storm"

var TRIDENT_HELPERS = []storm.Helper{
	&AbUpdateHelper{},
	&BootMetricsHelper{},
	&CheckJournaldHelper{},
	&CheckSelinuxHelper{},
	&CheckSshHelper{},
	&DisplayLogsHelper{},
	&ManualRollbackHelper{},
	&PrepareImages{},
	&ProcessMetricsHelper{},
	&RebuildRaidHelper{},
	&WaitForLoginHelper{},
	&DirectStreamingHelper{},
}
