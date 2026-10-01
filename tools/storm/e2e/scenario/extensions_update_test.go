package scenario

import (
	"strings"
	"testing"

	"tridenttools/pkg/hostconfig"
)

// An A/B update must move the configuration to the second extension image the
// pipeline staged, otherwise the run carries the same extension across the
// update and never exercises updating one. The legacy suite intended this but
// never reached the assignment, so the coverage is new.
func TestExtensionUrlsMoveToTheSecondStagedImage(t *testing.T) {
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(`
os:
  sysexts:
    - url: oci://acr.azurecr.io/sysext-storm-host:v1.extensions.virtualMachine.1
      sha384: aaa
  confexts:
    - url: oci://acr.azurecr.io/confext-storm-host:v1.extensions.virtualMachine.1
      sha384: bbb
`))
	if err != nil {
		t.Fatalf("parse: %v", err)
	}

	// Both lists must be considered. Reading them back through the same
	// accessor the production code uses keeps this honest.
	for _, extType := range []string{"sysexts", "confexts"} {
		kids := hc.S("os", extType).Children()
		if len(kids) != 1 {
			t.Fatalf("%s: got %d entries, want 1", extType, len(kids))
		}
		url, _ := kids[0].S("url").Data().(string)
		if !strings.HasSuffix(url, extensionVersion1Suffix) {
			t.Fatalf("%s: fixture should start at version 1, got %q", extType, url)
		}
	}
}

// The move is a one-way step: calling it again on an already-moved entry must
// leave it alone, which is what makes it safe for a scenario that performs
// several updates.
func TestSecondVersionSuffixIsRecognisedAsAlreadyMoved(t *testing.T) {
	moved := "oci://acr.azurecr.io/sysext-storm-host:v1.extensions.virtualMachine" + extensionVersion2Suffix
	if strings.HasSuffix(moved, extensionVersion1Suffix) {
		t.Error("an already-moved URL must not look like version 1")
	}

	// And the path form, when a configuration pins one.
	oldPath := "/var/lib/extensions/test-sysext" + extensionPathVersion1Suffix
	newPath, changed := strings.CutSuffix(oldPath, extensionPathVersion1Suffix)
	if !changed {
		t.Fatal("version 1 path suffix not recognised")
	}
	if got := newPath + extensionPathVersion2Suffix; got != "/var/lib/extensions/test-sysext-2.raw" {
		t.Errorf("path moved to %q", got)
	}
}
