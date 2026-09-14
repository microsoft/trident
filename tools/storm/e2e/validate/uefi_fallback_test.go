package validate

import (
	"testing"

	"tridenttools/pkg/hostconfig"
)

func specFromYaml(t *testing.T, yaml string) hostconfig.HostConfig {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(yaml))
	if err != nil {
		t.Fatalf("parse spec: %v", err)
	}
	return hc
}

// The schema spells a mount point either as a bare path or as an object with a
// `path`, and the checked-in configurations use both forms in the same file.
func TestEspMountPointAcceptsBothMountPointForms(t *testing.T) {
	object := specFromYaml(t, `
storage:
  filesystems:
  - deviceId: home
    mountPoint: /home
  - deviceId: esp
    mountPoint:
      path: /boot/efi
`)
	if got, ok := EspMountPoint(object); !ok || got != "/boot/efi" {
		t.Errorf("object form: got %q (ok=%v), want /boot/efi", got, ok)
	}

	bare := specFromYaml(t, `
storage:
  filesystems:
  - deviceId: esp
    mountPoint: /boot/efi
`)
	if got, ok := EspMountPoint(bare); !ok || got != "/boot/efi" {
		t.Errorf("bare form: got %q (ok=%v), want /boot/efi", got, ok)
	}
}

// An unresolvable ESP must be reported, not skipped: "cannot check" looking
// like "checked and fine" is what made this validator vacuous.
func TestEspMountPointReportsWhenAbsent(t *testing.T) {
	for name, yaml := range map[string]string{
		"no esp filesystem": "storage:\n  filesystems:\n  - deviceId: home\n    mountPoint: /home\n",
		"no filesystems":    "storage: {}\n",
		"empty mount point": "storage:\n  filesystems:\n  - deviceId: esp\n    mountPoint: \"\"\n",
	} {
		t.Run(name, func(t *testing.T) {
			if _, ok := EspMountPoint(specFromYaml(t, yaml)); ok {
				t.Error("expected the ESP mount point to be reported as unresolvable")
			}
		})
	}
}

// Only `disabled` is asserted over SSH; the phase-dependent modes belong to the
// injected health check.
func TestValidateUefiFallbackOnlyChecksDisabled(t *testing.T) {
	for _, mode := range []string{"conservative", "optimistic", ""} {
		yaml := "storage:\n  filesystems:\n  - deviceId: esp\n    mountPoint: /boot/efi\n"
		if mode != "" {
			yaml = "os:\n  uefiFallback: " + mode + "\n" + yaml
		}

		var sa SoftAsserter
		// A nil client is safe precisely because these modes return before
		// running any command.
		ValidateUefiFallback(&sa, nil, specFromYaml(t, yaml))

		if sa.Failures() != 0 {
			t.Errorf("mode %q recorded failures: %v", mode, sa.Err())
		}
	}
}

// A configuration with uefiFallback disabled but no resolvable ESP must fail
// rather than quietly pass.
func TestValidateUefiFallbackFailsWithoutAnEsp(t *testing.T) {
	spec := specFromYaml(t, "os:\n  uefiFallback: disabled\nstorage:\n  filesystems: []\n")

	var sa SoftAsserter
	ValidateUefiFallback(&sa, nil, spec)

	if !sa.HasFailures() {
		t.Error("expected a failure when the ESP mount point cannot be resolved")
	}
}
