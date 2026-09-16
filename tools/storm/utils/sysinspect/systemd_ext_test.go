package sysinspect

import "testing"

const sysextStatusSample = `[
  {"hierarchy":"/opt","extensions":["myext"]},
  {"hierarchy":"/usr","extensions":["myext","other"]},
  {"hierarchy":"/var","extensions":null}
]`

func TestParseSystemdExtStatus(t *testing.T) {
	active, err := ParseSystemdExtStatus(sysextStatusSample)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(active) != 2 {
		t.Fatalf("got %d active exts, want 2 (deduped)", len(active))
	}
	for _, want := range []string{"myext", "other"} {
		if _, ok := active[want]; !ok {
			t.Errorf("missing active ext %q", want)
		}
	}
}

// TestParseSystemdExtStatus_ScalarExtensions covers the systemd variant where
// the `extensions` field is a bare string ("none" for empty hierarchies, or a
// single extension name) rather than an array.
func TestParseSystemdExtStatus_ScalarExtensions(t *testing.T) {
	const sample = `[
  {"hierarchy":"/opt","extensions":"none"},
  {"hierarchy":"/usr","extensions":"solo"}
]`
	active, err := ParseSystemdExtStatus(sample)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(active) != 1 {
		t.Fatalf("got %d active exts, want 1 (none filtered)", len(active))
	}
	if _, ok := active["solo"]; !ok {
		t.Errorf("missing active ext %q", "solo")
	}
	if _, ok := active["none"]; ok {
		t.Error(`"none" sentinel should not be treated as an active extension`)
	}
}

// extType forms the binary name in the remote command, so shell quoting cannot
// contain it; the value has to be rejected outright. A nil client is fine here
// because validation happens before the command is built.
func TestSystemdExtStatusRejectsUnknownExtensionType(t *testing.T) {
	for _, extType := range []string{"", "sysext; rm -rf /", "SYSEXT", "portable"} {
		t.Run(extType, func(t *testing.T) {
			if _, err := SystemdExtStatus(nil, extType); err == nil {
				t.Errorf("SystemdExtStatus(%q) = nil error, want rejection", extType)
			}
		})
	}
}
