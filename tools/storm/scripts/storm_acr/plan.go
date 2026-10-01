package storm_acr

import (
	"fmt"
	"path/filepath"
)

// maxOciImageVersion is how many versions of an OCI-hosted OS image are
// staged. It must cover the highest version the scenario can step to, which is
// the split ring's (maxSplitRingImageVersion in the scenario package); split
// A/B update runs from the 'ci' ring upwards.
const maxOciImageVersion = 4

// Plan is what a Trident configuration hosts in ACR.
type Plan struct {
	Repository string
	Files      []string
	// EmitUrl is set only for a configuration that INSTALLS from the pushed
	// image. Emitting it for extensions would point image.url at a system
	// extension and replace the OS image entirely.
	EmitUrl bool
	// EmitRepo is set only for a configuration that CONSUMES system extensions
	// from the repository. The scenario reads a non-empty sysext repo as "this
	// run has extensions", so setting it elsewhere makes host-config
	// preparation look for a sysext file that was never built.
	EmitRepo bool
}

// planFor resolves what a configuration needs hosted in ACR, or false when it
// needs nothing. Deciding here means the pipeline can invoke the push and the
// cleanup unconditionally for every configuration, with no shell branch.
//
//   - extensions installs system extension images from ACR.
//   - misc installs its COSI from an OCI registry rather than over HTTP, and is
//     the only coverage of the OCI image source. Ported from the legacy suite's
//     trident-prep.yml, which passed --ociCosiUrl for misc alone.
func planFor(config, runtimeEnv, sourceDir string) (Plan, bool) {
	switch config {
	case "extensions":
		return Plan{
			Repository: "sysext-storm-" + runtimeEnv,
			Files: []string{
				filepath.Join(sourceDir, "test-sysext-1.raw"),
				filepath.Join(sourceDir, "test-sysext-2.raw"),
			},
			EmitRepo: true,
		}, true
	case "misc":
		files := make([]string, 0, maxOciImageVersion)
		for v := 1; v <= maxOciImageVersion; v++ {
			files = append(files, filepath.Join(sourceDir, "artifacts", "test-image", ociImageName(v)))
		}
		return Plan{
			Repository: "cosi-storm-" + runtimeEnv,
			Files:      files,
			EmitUrl:    true,
		}, true
	default:
		return Plan{}, false
	}
}

// ociImageName is the on-disk name of version v, matching the convention
// prepare-images uses: v1 is the bare name, later versions carry a _vN suffix.
func ociImageName(v int) string {
	if v == 1 {
		return "regular.cosi"
	}
	return fmt.Sprintf("regular_v%d.cosi", v)
}

// TagBase namespaces a run's images so concurrent builds and the legacy suite
// cannot collide.
func TagBase(buildId, config, deploymentEnvironment string) string {
	return fmt.Sprintf("v%s.%s.%s", buildId, config, deploymentEnvironment)
}
