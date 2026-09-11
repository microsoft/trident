package validate

import (
	"fmt"
	"path/filepath"
	"strings"

	"golang.org/x/crypto/ssh"

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

// HasExtensions reports whether the Host Status spec declares any system
// extensions (sysexts or confexts), used to self-select the extensions
// validation.
func HasExtensions(hs tridentutil.HostStatus) bool {
	os := hs.Spec().S("os")
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
func ValidateExtensions(sa *SoftAsserter, client *ssh.Client, hs tridentutil.HostStatus) {
	os := hs.Spec().S("os")

	for listKey, extType := range extensionKinds {
		configured := os.S(listKey).Children()
		if len(configured) == 0 {
			continue
		}

		active, err := sysinspect.SystemdExtStatus(client, extType)
		if err != nil {
			sa.Fail(fmt.Sprintf("extensions/%s-status", extType), err)
			continue
		}

		for _, ext := range configured {
			path, ok := ext.S("path").Data().(string)
			if !ok {
				sa.Failf(fmt.Sprintf("extensions/%s-path", extType),
					"configured %s entry has no path", extType)
				continue
			}

			// Verify the extension path exists on the target OS.
			out, err := sshutils.RunCommand(client, fmt.Sprintf("test -e %s", path))
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
