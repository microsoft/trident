package config

import "fmt"

// Flavor identifies the image variant under test.
//
// The pipeline previously derived the skip/uki/secure-boot flags from this
// value with a chain of bash conditionals spread across two YAML templates.
// The mapping lives here instead, so a single --flavor argument fully
// determines the test profile and the same profile applies to local runs.
type Flavor string

const (
	FlavorQemuGrub Flavor = "qemu-grub"
	FlavorQemu     Flavor = "qemu"
	FlavorUki      Flavor = "uki"
)

// FlavorProfile is the set of test behaviors implied by a Flavor.
type FlavorProfile struct {
	SkipExtensionTesting      bool
	SkipRuntimeUpdates        bool
	SkipNetplanRuntimeTesting bool
	Uki                       bool

	// SupportsSecureBoot reports whether secure boot may be enabled for this
	// flavor. Note this is deliberately not the same as !Uki: it mirrors the
	// pipeline's long-standing behavior of gating secure boot on the flavor
	// name alone, so the "qemu" flavor keeps secure boot even though it sets
	// Uki. Preserved as-is to avoid changing what CI exercises.
	SupportsSecureBoot bool
}

// Profile returns the behaviors implied by f, or an error if f is unknown.
func (f Flavor) Profile() (FlavorProfile, error) {
	switch f {
	case FlavorQemuGrub:
		return FlavorProfile{
			SupportsSecureBoot: true,
		}, nil

	case FlavorQemu:
		// Root-verity image: extensions cannot be added to the qcow2 and the
		// netplan config cannot be modified on a verity rootfs, so a runtime
		// update would have nothing to service.
		return FlavorProfile{
			SkipExtensionTesting:      true,
			SkipRuntimeUpdates:        true,
			SkipNetplanRuntimeTesting: true,
			Uki:                       true,
			SupportsSecureBoot:        true,
		}, nil

	case FlavorUki:
		return FlavorProfile{
			Uki: true,
		}, nil
	}

	return FlavorProfile{}, fmt.Errorf("unknown image flavor %q", f)
}

// ApplyFlavor folds the flavor profile into c and returns the profile.
//
// Skips are additive: a flavor may only add a skip, never clear one the caller
// asked for explicitly. This keeps flags like --skip-manual-rollbacks usable on
// top of any flavor.
func (c *TestConfig) ApplyFlavor() (FlavorProfile, error) {
	profile, err := Flavor(c.Flavor).Profile()
	if err != nil {
		return FlavorProfile{}, err
	}

	c.SkipExtensionTesting = c.SkipExtensionTesting || profile.SkipExtensionTesting
	c.SkipRuntimeUpdates = c.SkipRuntimeUpdates || profile.SkipRuntimeUpdates
	c.SkipNetplanRuntimeTesting = c.SkipNetplanRuntimeTesting || profile.SkipNetplanRuntimeTesting
	c.Uki = c.Uki || profile.Uki

	return profile, nil
}
