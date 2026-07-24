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
