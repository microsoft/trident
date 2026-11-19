package validate

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"golang.org/x/crypto/ssh"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/sysinspect"
)

func specFromYaml(t *testing.T, yaml string) hostconfig.HostConfig {
	t.Helper()
	hc, err := hostconfig.NewHostConfigFromYaml([]byte(yaml))
	if err != nil {
		t.Fatalf("parse spec: %v", err)
	}
	return hc
}

// Every configuration shipped in the repo must resolve, because a
// configuration whose ESP cannot be found fails validation. This is the guard
// that was missing: the ESP used to be located by device ID, and the RAID
// configurations call theirs "esp1", so three of them failed in build 1203859.
//
// This reads the tracked configurations under tests/e2e_tests rather than the
// copies under tools/storm/e2e/configurations: the latter are produced by
// `go generate` and are gitignored, so globbing them makes the test depend on
// generated state that a clean checkout does not have.
func TestEspMountPointResolvesForEveryCheckedInConfig(t *testing.T) {
	configs, err := filepath.Glob(filepath.Join("..", "..", "..", "..",
		"tests", "e2e_tests", "trident_configurations", "*", "trident-config.yaml"))
	if err != nil {
		t.Fatalf("glob configurations: %v", err)
	}
	if len(configs) < 10 {
		t.Fatalf("expected the checked-in configurations, found %d", len(configs))
	}

	for _, cfg := range configs {
		name := filepath.Base(filepath.Dir(cfg))
		t.Run(name, func(t *testing.T) {
			raw, err := os.ReadFile(cfg)
			if err != nil {
				t.Fatalf("read: %v", err)
			}
			spec, err := hostconfig.NewHostConfigFromYaml(raw)
			if err != nil {
				t.Fatalf("parse: %v", err)
			}

			got, ok := EspMountPoint(spec)
			if !ok {
				t.Fatalf("could not resolve the ESP mount point")
			}
			if got != "/boot/efi" {
				t.Errorf("ESP mount point = %q, want /boot/efi", got)
			}
		})
	}
}

// The schema spells a mount point either as a bare path or as an object with a
// `path`, and the checked-in configurations use both forms in the same file.
func TestEspMountPointAcceptsBothMountPointForms(t *testing.T) {
	object := specFromYaml(t, `
storage:
  disks:
  - partitions:
    - id: esp
      type: esp
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
  disks:
  - partitions:
    - id: esp1
      type: esp
  filesystems:
  - deviceId: esp1
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
		"no esp partition":  "storage:\n  filesystems:\n  - deviceId: home\n    mountPoint: /home\n",
		"no filesystems":    "storage: {}\n",
		"empty mount point": "storage:\n  disks:\n  - partitions:\n    - id: esp\n      type: esp\n  filesystems:\n  - deviceId: esp\n    mountPoint: \"\"\n",
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
		yaml := "storage:\n  disks:\n  - partitions:\n    - id: esp\n      type: esp\n  filesystems:\n  - deviceId: esp\n    mountPoint: /boot/efi\n"
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

func TestValidateUefiFallbackDisabledRequiresMountedVfatEsp(t *testing.T) {
	for _, tc := range []struct {
		name       string
		rows       []sysinspect.FindmntRow
		findmntErr error
	}{
		{
			name:       "not mounted",
			findmntErr: errors.New("findmnt: /boot/efi is not a mountpoint"),
		},
		{
			name: "wrong filesystem",
			rows: []sysinspect.FindmntRow{{Target: "/boot/efi", FsType: "ext4"}},
		},
		{
			name: "wrong target",
			rows: []sysinspect.FindmntRow{{Target: "/boot", FsType: espFindmntFsType}},
		},
		{
			name: "ambiguous rows",
			rows: []sysinspect.FindmntRow{
				{Target: "/boot/efi", FsType: espFindmntFsType},
				{Target: "/boot/efi", FsType: espFindmntFsType},
			},
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var ranFallbackProbe bool
			var sa SoftAsserter
			validateUefiFallbackDisabled(
				&sa,
				nil,
				"/boot/efi",
				func(*ssh.Client, string) ([]sysinspect.FindmntRow, error) {
					return tc.rows, tc.findmntErr
				},
				func(*ssh.Client, string) (*sshutils.SshCmdOutput, error) {
					ranFallbackProbe = true
					return &sshutils.SshCmdOutput{Stdout: "NO_FALLBACK_DIR\n"}, nil
				})

			summary := sa.Summary()
			if ranFallbackProbe {
				t.Fatalf("fallback directory probe ran before ESP mount was verified:\n%s", summary)
			}
			if !sa.HasFailures() {
				t.Fatalf("expected ESP mount failure:\n%s", summary)
			}
			if strings.Contains(summary, "PASS  uefi/disabled") {
				t.Fatalf("unmounted or wrong ESP was reported as disabled fallback success:\n%s", summary)
			}
		})
	}
}

func TestValidateUefiFallbackDisabledPassesWithoutFallbackDirOnMountedEsp(t *testing.T) {
	var ranFallbackProbe bool
	var sa SoftAsserter
	validateUefiFallbackDisabled(
		&sa,
		nil,
		"/boot/efi",
		func(*ssh.Client, string) ([]sysinspect.FindmntRow, error) {
			return []sysinspect.FindmntRow{{Target: "/boot/efi", FsType: espFindmntFsType}}, nil
		},
		func(*ssh.Client, string) (*sshutils.SshCmdOutput, error) {
			ranFallbackProbe = true
			return &sshutils.SshCmdOutput{Stdout: "NO_FALLBACK_DIR\n"}, nil
		})

	summary := sa.Summary()
	if !ranFallbackProbe {
		t.Fatalf("fallback directory probe did not run after ESP mount was verified:\n%s", summary)
	}
	if sa.HasFailures() {
		t.Fatalf("unexpected failures:\n%s", summary)
	}
	for _, want := range []string{"PASS  uefi/esp-mounted", "PASS  uefi/disabled"} {
		if !strings.Contains(summary, want) {
			t.Fatalf("missing %q:\n%s", want, summary)
		}
	}
}

func TestValidateUefiFallbackDisabledChecksProbeStatus(t *testing.T) {
	var sa SoftAsserter
	validateUefiFallbackDisabled(
		&sa,
		nil,
		"/boot/efi",
		func(*ssh.Client, string) ([]sysinspect.FindmntRow, error) {
			return []sysinspect.FindmntRow{{Target: "/boot/efi", FsType: espFindmntFsType}}, nil
		},
		func(*ssh.Client, string) (*sshutils.SshCmdOutput, error) {
			return &sshutils.SshCmdOutput{Stdout: "NO_FALLBACK_DIR\n", Status: 7}, nil
		})

	summary := sa.Summary()
	if !sa.HasFailures() {
		t.Fatalf("expected non-zero probe status to fail:\n%s", summary)
	}
	if strings.Contains(summary, "PASS  uefi/disabled") {
		t.Fatalf("non-zero probe status was reported as success:\n%s", summary)
	}
}

// Trident decides which filesystem is the ESP from overrideEspMount, not from
// the partition type, so the validator has to honour it or it will probe the
// wrong path (or refuse to run) on configurations that set it.
func TestEspMountPointHonoursOverrideEspMount(t *testing.T) {
	const spec = `
storage:
  disks:
    - partitions:
        - id: esp
          type: esp
        - id: other
          type: linux-generic
  filesystems:
    - deviceId: esp
      mountPoint: /boot/efi
    - deviceId: other
      mountPoint: /srv/esp
`
	t.Run("block disclaims an esp-typed partition", func(t *testing.T) {
		blocked := specFromYaml(t, `
storage:
  disks:
    - partitions:
        - id: esp
          type: esp
  filesystems:
    - deviceId: esp
      mountPoint: /boot/efi
      overrideEspMount: block
`)
		if got, ok := EspMountPoint(blocked); ok {
			t.Errorf("blocked filesystem was treated as the ESP: %q", got)
		}
	})

	t.Run("override marks a non-esp partition", func(t *testing.T) {
		overridden := specFromYaml(t, `
storage:
  disks:
    - partitions:
        - id: other
          type: linux-generic
  filesystems:
    - deviceId: other
      mountPoint: /srv/esp
      overrideEspMount: override
`)
		got, ok := EspMountPoint(overridden)
		if !ok || got != "/srv/esp" {
			t.Errorf("EspMountPoint = (%q, %v), want (/srv/esp, true)", got, ok)
		}
	})

	t.Run("default still uses the partition type", func(t *testing.T) {
		got, ok := EspMountPoint(specFromYaml(t, spec))
		if !ok || got != "/boot/efi" {
			t.Errorf("EspMountPoint = (%q, %v), want (/boot/efi, true)", got, ok)
		}
	})
}

// Trident's default rule keys on the mount point, not the partition type: a
// filesystem at /boot/efi is the ESP even when its partition is not esp-typed
// (adopted filesystems, for instance). Requiring the partition type made such
// configurations unresolvable, which the disabled-fallback check reports as a
// failure.
func TestEspMountPointUsesDefaultMountPointRule(t *testing.T) {
	adopted := specFromYaml(t, `
storage:
  disks:
    - partitions:
        - id: other
          type: linux-generic
  filesystems:
    - deviceId: other
      mountPoint: /boot/efi
`)
	got, ok := EspMountPoint(adopted)
	if !ok || got != "/boot/efi" {
		t.Errorf("EspMountPoint = (%q, %v), want (/boot/efi, true)", got, ok)
	}

	// block still wins over the default mount rule.
	blocked := specFromYaml(t, `
storage:
  filesystems:
    - deviceId: other
      mountPoint: /boot/efi
      overrideEspMount: block
`)
	if got, ok := EspMountPoint(blocked); ok {
		t.Errorf("blocked /boot/efi treated as ESP: %q", got)
	}
}
