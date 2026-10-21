package proxies

import (
	"bytes"
	"encoding/base64"
	"encoding/xml"
	"fmt"
	"strings"
	"testing"

	"github.com/sirupsen/logrus"
	logrustest "github.com/sirupsen/logrus/hooks/test"
)

var testMetadataSHA384 = strings.Repeat("ab", 48)
var testMetadataSHA384Base64 = base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0xab}, 48))

type labeledOmahaPackage struct {
	Name     string `xml:"name,attr"`
	Hash     string `xml:"hash,attr"`
	SHA256   string `xml:"hash_sha256,attr"`
	Size     string `xml:"size,attr"`
	Required bool   `xml:"required,attr"`
	Labels   []struct {
		Key   string `xml:"key,attr"`
		Value string `xml:"value,attr"`
	} `xml:"labels>label"`
}

func omahaPackages(t *testing.T, body string) []labeledOmahaPackage {
	t.Helper()
	var response struct {
		Packages []labeledOmahaPackage `xml:"app>updatecheck>manifest>packages>package"`
	}
	if err := xml.Unmarshal([]byte(body), &response); err != nil {
		t.Fatalf("decode adapted response: %v", err)
	}
	return response.Packages
}

func assertCosiMetadataLabel(t *testing.T, body, name, digest string) {
	t.Helper()
	for _, pkg := range omahaPackages(t, body) {
		if pkg.Name != name {
			continue
		}
		if pkg.Hash != "" {
			t.Fatalf("COSI package still has a legacy hash: %q", pkg.Hash)
		}
		if len(pkg.Labels) != 1 || pkg.Labels[0].Key != "cosi.metadata.sha384" || pkg.Labels[0].Value != digest {
			t.Fatalf("unexpected COSI labels: %+v", pkg.Labels)
		}
		return
	}
	t.Fatalf("response has no package %q: %s", name, body)
}

func TestLabelCosiPackage(t *testing.T) {
	wrap := func(packages string) string {
		return `<response protocol="3.0"><daystart elapsed_seconds="123"/><app appid="test" status="ok"><updatecheck status="ok"><urls><url codebase="https://example.invalid/a?x=1&amp;y=2"/></urls><manifest version="5.0.0"><packages>` +
			packages + `</packages><actions><action event="postinstall" run="true"/></actions></manifest></updatecheck><event status="ok"/></app></response>`
	}
	cosi := fmt.Sprintf(`<package name="acl.cosi" hash="%s" hash_sha256="other-digest" size="1234" required="true"/>`, testMetadataSHA384Base64)

	t.Run("labels-only-and-preserves-backend-fields", func(t *testing.T) {
		input := wrap(cosi + `<package name="extra.txt" hash="extra-hash" size="99"/>`)
		output, err := labelCosiPackage([]byte(input), "")
		if err != nil {
			t.Fatal(err)
		}
		body := string(output)
		assertCosiMetadataLabel(t, body, "acl.cosi", testMetadataSHA384)
		packages := omahaPackages(t, body)
		if len(packages) != 2 || packages[0].Size != "1234" || !packages[0].Required || packages[0].SHA256 != "other-digest" {
			t.Fatalf("package attributes changed: %+v", packages)
		}
		if packages[1].Name != "extra.txt" || packages[1].Hash != "extra-hash" || len(packages[1].Labels) != 0 {
			t.Fatalf("extra package changed: %+v", packages[1])
		}
		for _, expected := range []string{`protocol="3.0"`, `elapsed_seconds="123"`, `appid="test"`, `version="5.0.0"`, `codebase="https://example.invalid/a?x=1&amp;y=2"`, `<event status="ok">`, `<action event="postinstall" run="true">`} {
			if !strings.Contains(body, expected) {
				t.Errorf("response lost %s: %s", expected, body)
			}
		}
	})

	t.Run("custom-package-name-and-escaping", func(t *testing.T) {
		input := wrap(strings.Replace(cosi, `name="acl.cosi"`, `name="custom&amp;image.cosi"`, 1))
		output, err := labelCosiPackage([]byte(input), "custom&image.cosi")
		if err != nil {
			t.Fatal(err)
		}
		assertCosiMetadataLabel(t, string(output), "custom&image.cosi", testMetadataSHA384)
	})

	for name, input := range map[string]string{
		"no-update":  `<response><app><updatecheck status="noupdate"/></app></response>`,
		"event-only": `<response><app><event status="ok"/></app></response>`,
	} {
		t.Run(name, func(t *testing.T) {
			output, err := labelCosiPackage([]byte(input), "")
			if err != nil {
				t.Fatal(err)
			}
			if string(output) != input {
				t.Fatalf("response changed: %s", output)
			}
		})
	}

	for name, input := range map[string]string{
		"malformed-xml":    `<response><app></response>`,
		"missing-hash":     wrap(`<package name="acl.cosi"/>`),
		"empty-hash":       wrap(`<package name="acl.cosi" hash=""/>`),
		"invalid-base64":   wrap(`<package name="acl.cosi" hash="not-valid-base64!!"/>`),
		"short-hash":       wrap(fmt.Sprintf(`<package name="acl.cosi" hash="%s"/>`, base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0xab}, 47)))),
		"oversized-hash":   wrap(fmt.Sprintf(`<package name="acl.cosi" hash="%s"/>`, base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0xab}, 49)))),
		"missing-package":  wrap(`<package name="wrong.cosi" hash="digest"/>`),
		"duplicate-target": wrap(cosi + cosi),
	} {
		t.Run(name, func(t *testing.T) {
			if output, err := labelCosiPackage([]byte(input), ""); err == nil {
				t.Fatalf("expected adaptation to fail, got: %s", output)
			}
		})
	}
}

func TestLabelCosiPackageLogging(t *testing.T) {
	logger := logrus.StandardLogger()
	previousHooks := logger.ReplaceHooks(make(logrus.LevelHooks))
	previousLevel := logger.GetLevel()
	logger.SetLevel(logrus.InfoLevel)
	hook := logrustest.NewGlobal()
	t.Cleanup(func() {
		logger.ReplaceHooks(previousHooks)
		logger.SetLevel(previousLevel)
	})

	pkg := fmt.Sprintf(`<package name="acl.cosi" size="1234"><labels><label key="cosi.metadata.sha384" value="%s"></label></labels></package>`, testMetadataSHA384)
	inputPackage := fmt.Sprintf(`<package name="acl.cosi" hash="%s" size="1234"/>`, testMetadataSHA384Base64)
	wrap := func(packages string) string {
		return `<response><app><updatecheck><urls><url codebase="https://user:password@example.invalid/images/?sig=secret"/></urls><manifest><packages>` + packages + `</packages></manifest></updatecheck></app></response>`
	}
	for _, tc := range []struct {
		name     string
		response string
		wantLog  bool
	}{
		{"labeled-package-only", wrap(inputPackage + `<package name="extra.txt" hash="extra-hash"/>`), true},
		{"no-update", `<response><app><updatecheck status="noupdate"/></app></response>`, false},
		{"event-only", `<response><app><event status="ok"/></app></response>`, false},
		{"duplicate-target", wrap(inputPackage + inputPackage), false},
		{"malformed-after-package", strings.TrimSuffix(wrap(inputPackage), `</response>`), false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			hook.Reset()
			output, err := labelCosiPackage([]byte(tc.response), "")
			entries := hook.AllEntries()
			if !tc.wantLog {
				if len(entries) != 0 {
					t.Fatalf("unexpected package log: %+v", entries)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			expected := "Nebraska COSI package XML: " + pkg + "; metadata_sha384=" + strings.Repeat("ab", 48)
			if len(entries) != 1 || entries[0].Level != logrus.InfoLevel || entries[0].Message != expected {
				t.Fatalf("expected only the labeled package XML at INFO, got: %+v", entries)
			}
			if !bytes.Contains(output, []byte(pkg)) {
				t.Fatalf("logged package differs from the actual response: %s", output)
			}
		})
	}

	t.Run("matches-trident-hex-digest", func(t *testing.T) {
		hook.Reset()
		const encoded = "gibyPdJdlEOCtzjtn/RFaPDh9LzBALL6tRPldOl0PIQaoUD1eRbBJs+gZ/BJhW/z"
		const decoded = "8226f23dd25d944382b738ed9ff44568f0e1f4bcc100b2fab513e574e9743c841aa140f57916c126cfa067f049856ff3"
		input := wrap(strings.Replace(inputPackage, testMetadataSHA384Base64, encoded, 1))
		output, err := labelCosiPackage([]byte(input), "")
		if err != nil {
			t.Fatal(err)
		}
		assertCosiMetadataLabel(t, string(output), "acl.cosi", decoded)
		expectedPackage := strings.Replace(pkg, testMetadataSHA384, decoded, 1)
		expected := "Nebraska COSI package XML: " + expectedPackage + "; metadata_sha384=" + decoded
		entries := hook.AllEntries()
		if len(entries) != 1 || entries[0].Level != logrus.InfoLevel || entries[0].Message != expected {
			t.Fatalf("expected package XML and matching Trident hex digest at INFO, got: %+v", entries)
		}
	})
}
