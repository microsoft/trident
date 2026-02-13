package helpers

import "github.com/microsoft/storm"

var TRIDENT_HELPERS = []storm.Helper{
	&AbUpdateHelper{},
	&BootMetricsHelper{},
	&CheckSelinuxHelper{},
	&CheckSshHelper{},
	&DisplayLogsHelper{},
	&ManualRollbackHelper{},
	&PrepareImages{},
	&RebuildRaidHelper{},
	&WaitForLoginHelper{},
	&DirectStreamingHelper{},
}
