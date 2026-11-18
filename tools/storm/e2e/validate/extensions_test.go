package validate

import (
	"fmt"
	"strings"
	"testing"

	"tridenttools/pkg/hostconfig"
	tridentutil "tridenttools/storm/utils/trident"
)

func TestValidateExtensionsRejectsMissingReportedData(t *testing.T) {
	for _, kind := range []string{"sysexts", "confexts"} {
		for _, tc := range []struct {
			name     string
			reported string
			check    string
		}{
			{"missing spec", "", "list"},
			{"missing list", "spec:\n  os: {}\n", "list"},
			{"wrong list type", "spec:\n  os:\n    %s: {}\n", "list"},
			{"empty list", "spec:\n  os:\n    %s: []\n", "count"},
			{"missing path", "spec:\n  os:\n    %s: [{}]\n", "path"},
			{"wrong path type", "spec:\n  os:\n    %s: [{path: 7}]\n", "path"},
			{"empty path", "spec:\n  os:\n    %s: [{path: ''}]\n", "path"},
			{"relative path", "spec:\n  os:\n    %s: [{path: test.raw}]\n", "path"},
			{"directory path", "spec:\n  os:\n    %s: [{path: /var/lib/extensions}]\n", "path"},
		} {
			t.Run(kind+"/"+tc.name, func(t *testing.T) {
				expected := extensionConfig(t, fmt.Sprintf("os:\n  %s:\n  - url: https://test/one.raw\n", kind))
				reported := tc.reported
				if strings.Contains(reported, "%s") {
					reported = fmt.Sprintf(reported, kind)
				}
				hs := extensionStatus(t, reported)
				var sa SoftAsserter
				ValidateExtensions(&sa, nil, hs, expected)
				check := "FAIL  extensions/" + extensionKinds[kind] + "-" + tc.check
				if !strings.Contains(sa.Summary(), check) {
					t.Fatalf("missing %s failure:\n%s", check, sa.Summary())
				}
			})
		}
	}
}

func TestValidateExtensionsRejectsDroppedOrDuplicateEntries(t *testing.T) {
	expected := extensionConfig(t, `
os:
  sysexts:
  - url: https://test/one.raw
  - url: https://test/two.raw
`)
	for _, reported := range []string{
		"spec:\n  os:\n    sysexts:\n    - path: /var/lib/extensions/one.raw\n",
		"spec:\n  os:\n    sysexts:\n    - path: /var/lib/extensions/one.raw\n    - path: /var/lib/extensions/one.raw\n",
	} {
		var sa SoftAsserter
		ValidateExtensions(&sa, nil, extensionStatus(t, reported), expected)
		if !sa.HasFailures() {
			t.Fatal("dropped or duplicated extension was accepted")
		}
	}
}

func TestValidateExtensionsSkipsUnconfiguredKinds(t *testing.T) {
	for _, config := range []string{"os: {}", "os: {sysexts: [], confexts: []}"} {
		var sa SoftAsserter
		ValidateExtensions(&sa, nil, extensionStatus(t, ""), extensionConfig(t, config))
		if sa.HasFailures() {
			t.Fatal(sa.Err())
		}
	}
}

func TestValidateExtensionsChecksResolvedPathsAfterUpdate(t *testing.T) {
	for _, kind := range []string{"sysexts", "confexts"} {
		t.Run(kind, func(t *testing.T) {
			extType := extensionKinds[kind]
			path := "/var/lib/extensions/test-2.raw"
			expected := extensionConfig(t, fmt.Sprintf("os:\n  %s:\n  - url: https://test/test-1.raw\n", kind))
			hs := extensionStatus(t, fmt.Sprintf("spec:\n  os:\n    %s:\n    - path: %s\n", kind, path))
			client := expectationTestClient(t, map[string]string{
				"sudo systemd-" + extType + " status --json=pretty --no-pager": `[{"hierarchy":"/usr","extensions":["test-2"]}]`,
				"test -e '" + path + "'": "",
			})
			var sa SoftAsserter
			ValidateExtensions(&sa, client, hs, expected)
			if sa.HasFailures() {
				t.Fatal(sa.Err())
			}
			if !strings.Contains(sa.Summary(), "PASS  extensions/"+extType+"-active") {
				t.Fatalf("no activation check ran:\n%s", sa.Summary())
			}
		})
	}
}

func extensionConfig(t *testing.T, source string) hostconfig.HostConfig {
	t.Helper()
	config, err := hostconfig.NewHostConfigFromYaml([]byte(source))
	if err != nil {
		t.Fatal(err)
	}
	return config
}

func extensionStatus(t *testing.T, source string) tridentutil.HostStatus {
	t.Helper()
	hs, err := tridentutil.NewHostStatusFromYaml([]byte(source))
	if err != nil {
		t.Fatal(err)
	}
	return hs
}

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
