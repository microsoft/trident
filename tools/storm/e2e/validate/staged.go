package validate

import (
	tridentutil "tridenttools/storm/utils/trident"
)

// ValidateAbUpdateStaged ports ab_update_staged_test.py::test_ab_update_staged.
// It confirms that, after staging an A/B update but before finalizing, the host
// reports the staged servicing state and that the active volume has not yet
// changed. abActive is the volume expected to still be active before finalize.
func ValidateAbUpdateStaged(
	sa *SoftAsserter,
	hs tridentutil.HostStatus,
	abActive tridentutil.AbVolumeSelection,
) {
	sa.Assert("ab-staged/servicing-state",
		hs.ServicingState() == tridentutil.ServicingStateAbUpdateStaged,
		"expected servicingState %q, got %q",
		tridentutil.ServicingStateAbUpdateStaged, hs.ServicingState())

	actual, present := hs.AbActiveVolume()
	sa.Assert("ab-staged/active-volume",
		present && actual == abActive,
		"expected abActiveVolume %q (unchanged), got %q (present=%v)", abActive, actual, present)
}
