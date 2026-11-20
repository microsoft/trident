// Package acl exercises manual adoption and A/B servicing of ACL guests.
package acl

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/microsoft/storm"
)

const (
	guestDir            = "/var/lib/trident/acl"
	NO_PREVIOUS_BOOT_ID = ""
)

// imageVariant selects the standard or nested-MSHV guest behavior.
type imageVariant string

const (
	variantStandard imageVariant = "standard"
	variantMSHV     imageVariant = "mshv"
)

// ACLScenario runs ordered adoption and A/B servicing cases against one guest.
type ACLScenario struct {
	args          scenarioArgs
	runner        *runner
	lifecycle     lifecycle
	failure       error
	logsCollected bool
	completed     bool
	finished      bool
}

// scenarioArgs defines local artifacts, evidence paths and guest operation limits.
type scenarioArgs struct {
	Variant          imageVariant  `help:"ACL image variant" enum:"standard,mshv" default:"standard"`
	ArtifactsDir     string        `help:"Directory containing ACL base.qcow2, update.cosi and id_rsa" required:""`
	OutputPath       string        `help:"Directory for test evidence" required:""`
	VMName           string        `help:"Name of the disposable QEMU VM" default:"acl"`
	BootTimeout      time.Duration `help:"Maximum wait for each SSH-ready boot" default:"10m"`
	OperationTimeout time.Duration `help:"Maximum duration of each deployment or guest operation" default:"30m"`
}

// Name identifies the ACL scenario to Storm.
func (s *ACLScenario) Name() string { return "acl" }

// Args exposes the scenario's command-line options.
func (s *ACLScenario) Args() any { return &s.args }

// Tags allows Storm to select ACL QEMU coverage.
func (s *ACLScenario) Tags() []string { return []string{"qemu", "acl"} }

// StagePaths leaves input staging to the supplied artifacts directory.
func (s *ACLScenario) StagePaths() []string { return nil }

// RequiredFiles defers input validation to the deployment case.
func (s *ACLScenario) RequiredFiles() []string { return nil }

// Setup leaves resource creation in deploy-vm so failures receive cleanup hooks.
func (s *ACLScenario) Setup(storm.SetupCleanupContext) error { return nil }

// Storm skips remaining cases after a failure, so cleanup also needs a hook.
func (s *ACLScenario) Cleanup(storm.SetupCleanupContext) error { return s.finish() }

// RegisterTestCases fixes the servicing order for cases sharing one guest.
func (s *ACLScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("deploy-vm", s.deployVm)
	r.RegisterTestCase("check-deployment", s.checkDeployment)
	r.RegisterTestCase("run-ab-update", s.runABUpdate)
	r.RegisterTestCase("run-rollback", s.runRollback)
	r.RegisterTestCase("run-ab-update-2", s.runABUpdate2)
	r.RegisterTestCase("run-ab-update-3", s.runABUpdate3)
	r.RegisterTestCase("check-persistence", s.checkPersistence)
	r.RegisterTestCase("collect-logs", s.collectLogs)
	r.RegisterTestCase("cleanup-vm", s.cleanupVm)
	return nil
}

// validateArtifacts requires a supported variant and nonempty image and SSH-key files.
func validateArtifacts(dir string, variant imageVariant) error {
	if variant != variantStandard && variant != variantMSHV {
		return fmt.Errorf("unsupported ACL variant %q; expected standard or mshv", variant)
	}
	for _, name := range []string{"base.qcow2", "update.cosi", "id_rsa"} {
		st, err := os.Stat(filepath.Join(dir, name))
		if err != nil {
			return err
		}
		if !st.Mode().IsRegular() || st.Size() == 0 {
			return fmt.Errorf("%s must be a nonempty regular file", name)
		}
	}
	return nil
}

// updateConfig selects the uploaded COSI for explicit staging and finalization.
func updateConfig() string {
	// These disposable fixtures exercise servicing, not checksum validation.
	return fmt.Sprintf("image:\n  url: file://%s/update.cosi\n  sha384: ignored\ninternalParams:\n  forceAbUpdate: true\n  noTransition: true\nos:\n  uefiFallback: conservative\n", guestDir)
}
