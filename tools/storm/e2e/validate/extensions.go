package validate

import (
	"fmt"
	"path/filepath"
	"strings"

	stormssh "github.com/microsoft/storm/utils/ssh"

	"tridenttools/pkg/hostconfig"
	"tridenttools/storm/utils/sshutils"
	"tridenttools/storm/utils/sysinspect"
	tridentutil "tridenttools/storm/utils/trident"
)

// extensionKinds maps a Host Configuration `os` extension list key to the
// systemd extension type used on the command line.
var extensionKinds = map[string]string{
	"sysexts":  "sysext",
	"confexts": "confext",
}

// HasExtensions reports whether the Host Configuration declares any system
// extensions (sysexts or confexts), used to self-select the extensions
// validation.
func HasExtensions(config hostconfig.HostConfig) bool {
	os := config.S("os")
	for key := range extensionKinds {
		if os.Exists(key) {
			return true
		}
	}
	return false
}

// ValidateExtensions ports extensions_test.py::test_extensions. For each
// configured sysext/confext it confirms the extension path exists on the host
// and that the extension is active per `systemd-<type> status`.
func ValidateExtensions(sa *SoftAsserter, client *stormssh.Client, hs tridentutil.HostStatus, expected hostconfig.HostConfig) {
	os := hs.Spec().S("os")

	for listKey, extType := range extensionKinds {
		requested := expected.S("os", listKey).Data()
		if requested == nil {
			continue
		}
		entries, ok := requested.([]any)
		if !ok {
			sa.Failf(fmt.Sprintf("extensions/%s-config", extType), "expected %s must be a list", listKey)
			continue
		}
		if len(entries) == 0 {
			continue
		}

		reported, ok := os.S(listKey).Data().([]any)
		if !ok {
			sa.Failf(fmt.Sprintf("extensions/%s-list", extType), "Host Status is missing a valid %s list", listKey)
			continue
		}
		if len(reported) != len(entries) {
			sa.Failf(fmt.Sprintf("extensions/%s-count", extType),
				"expected %d %s entries, Host Status reports %d", len(entries), listKey, len(reported))
			continue
		}

		configured := os.S(listKey).Children()
		paths := make([]string, 0, len(configured))
		seen := make(map[string]bool)
		for _, ext := range configured {
			path, ok := ext.S("path").Data().(string)
			if !ok || !filepath.IsAbs(path) || filepath.Ext(path) != ".raw" {
				sa.Failf(fmt.Sprintf("extensions/%s-path", extType),
					"reported %s entry has no valid absolute .raw path", extType)
				continue
			}
			if seen[path] {
				sa.Failf(fmt.Sprintf("extensions/%s-path", extType), "duplicate %s path %q", extType, path)
				continue
			}
			seen[path] = true
			paths = append(paths, path)
		}
		if len(paths) != len(entries) {
			continue
		}

		active, err := sysinspect.SystemdExtStatus(client, extType)
		if err != nil {
			sa.Fail(fmt.Sprintf("extensions/%s-status", extType), err)
			continue
		}

		for _, path := range paths {
			// Verify the extension path exists on the target OS.
			out, err := sshutils.RunCommand(client, fmt.Sprintf("test -e %s", sshutils.ShellQuote(path)))
			if err != nil {
				sa.Fail(fmt.Sprintf("extensions/%s-exists", extType), err)
			} else {
				sa.Assert(fmt.Sprintf("extensions/%s-exists", extType),
					out.Status == 0, "%s path does not exist: %s", extType, path)
			}

			// The active extension name is the file stem (basename minus its
			// final extension), matching Python's Path.stem.
			name := extensionStem(path)
			_, isActive := active[name]
			sa.Assert(fmt.Sprintf("extensions/%s-active", extType),
				isActive, "%s %q not found in 'systemd-%s status'", extType, name, extType)
		}
	}
}

// extensionStem returns the basename of a path with its final extension
// removed, matching Python's pathlib.Path.stem (e.g. "/x/foo.raw" -> "foo",
// "/x/foo.bar.raw" -> "foo.bar").
func extensionStem(path string) string {
	base := filepath.Base(path)
	ext := filepath.Ext(base)
	return strings.TrimSuffix(base, ext)
}
