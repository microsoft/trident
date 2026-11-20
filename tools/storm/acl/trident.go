// Package acl exercises manual adoption and A/B servicing of ACL guests.
package acl

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/microsoft/storm"
)

const guestDir = "/var/lib/trident/acl"

type imageVariant string

const (
	variantStandard imageVariant = "standard"
	variantMSHV     imageVariant = "mshv"
)

type ACLScenario struct {
	args          scenarioArgs
	runner        *runner
	lifecycle     lifecycle
	failure       error
	logsCollected bool
	completed     bool
	finished      bool
}

type scenarioArgs struct {
	Variant          imageVariant  `help:"ACL image variant" enum:"standard,mshv" default:"standard"`
	ArtifactsDir     string        `help:"Directory containing ACL base.qcow2, update.cosi, manifest.json and id_rsa" required:""`
	OutputPath       string        `help:"Directory for evidence and private VM working disk" required:""`
	VMName           string        `help:"Prefix for a uniquely named disposable QEMU VM" default:"acl"`
	BootTimeout      time.Duration `help:"Maximum wait for each SSH-ready boot" default:"10m"`
	OperationTimeout time.Duration `help:"Maximum duration of each deployment or guest operation" default:"30m"`
}

func (s *ACLScenario) Name() string                          { return "acl" }
func (s *ACLScenario) Args() any                             { return &s.args }
func (s *ACLScenario) Tags() []string                        { return []string{"qemu", "acl"} }
func (s *ACLScenario) StagePaths() []string                  { return nil }
func (s *ACLScenario) RequiredFiles() []string               { return nil }
func (s *ACLScenario) Setup(storm.SetupCleanupContext) error { return nil }

// Storm skips remaining cases after a failure, so cleanup also needs a hook.
func (s *ACLScenario) Cleanup(storm.SetupCleanupContext) error { return s.finish() }

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

type artifactInfo struct {
	Variant imageVariant `json:"variant"`
}

func readArtifacts(dir string, variant imageVariant) (artifactInfo, error) {
	info := artifactInfo{Variant: variant}
	if variant != variantStandard && variant != variantMSHV {
		return info, fmt.Errorf("unsupported ACL variant %q; expected standard or mshv", variant)
	}
	for _, name := range []string{"base.qcow2", "update.cosi", "manifest.json", "id_rsa"} {
		st, err := os.Stat(filepath.Join(dir, name))
		if err != nil {
			return info, err
		}
		if !st.Mode().IsRegular() || st.Size() == 0 {
			return info, fmt.Errorf("%s must be a nonempty regular file", name)
		}
	}
	var manifest struct {
		ImageSource string       `json:"imageSource"`
		Variant     imageVariant `json:"variant"`
	}
	manifestBytes, err := os.ReadFile(filepath.Join(dir, "manifest.json"))
	if err != nil {
		return info, err
	}
	if err = json.Unmarshal(manifestBytes, &manifest); err != nil {
		return info, err
	}
	if manifest.ImageSource != "acl-t" || manifest.Variant != variant {
		return info, fmt.Errorf("manifest must identify ACL-T %s", variant)
	}
	return info, nil
}

func updateConfig() string {
	// These disposable fixtures exercise servicing, not checksum validation.
	return fmt.Sprintf("image:\n  url: file://%s/update.cosi\n  sha384: ignored\ninternalParams:\n  forceAbUpdate: true\n  noTransition: true\nos:\n  uefiFallback: conservative\n", guestDir)
}
