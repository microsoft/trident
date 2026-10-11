package validate

import "testing"

func TestExtensionStem(t *testing.T) {
	cases := map[string]string{
		"/var/lib/ext/foo.raw":     "foo",
		"/var/lib/ext/foo.bar.raw": "foo.bar",
		"foo":                      "foo",
		"/a/b/c.sysext.raw":        "c.sysext",
	}
	for in, want := range cases {
		if got := extensionStem(in); got != want {
			t.Errorf("extensionStem(%q) = %q, want %q", in, got, want)
		}
	}
}
