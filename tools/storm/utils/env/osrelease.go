package env

import (
	"os"
	"reflect"
	"strings"
)

type OsReleaseInfo struct {
	Name             string `ini:"NAME"`
	Id               string `ini:"ID"`
	IdLike           string `ini:"ID_LIKE"`
	PrettyName       string `ini:"PRETTY_NAME"`
	CpeName          string `ini:"CPE_NAME"`
	Variant          string `ini:"VARIANT"`
	VariantId        string `ini:"VARIANT_ID"`
	Version          string `ini:"VERSION"`
	VersionId        string `ini:"VERSION_ID"`
	VersionCodename  string `ini:"VERSION_CODENAME"`
	BuildId          string `ini:"BUILD_ID"`
	ImageId          string `ini:"IMAGE_ID"`
	ImageVersion     string `ini:"IMAGE_VERSION"`
	ReleaseType      string `ini:"RELEASE_TYPE"`
	HomeUrl          string `ini:"HOME_URL"`
	DocumentationUrl string `ini:"DOCUMENTATION_URL"`
	SupportUrl       string `ini:"SUPPORT_URL"`
	BugReportUrl     string `ini:"BUG_REPORT_URL"`
	PrivacyPolicyUrl string `ini:"PRIVACY_POLICY_URL"`
	SupportEnd       string `ini:"SUPPORT_END"`
	Logo             string `ini:"LOGO"`
	AnsiColor        string `ini:"ANSI_COLOR"`
	VendorName       string `ini:"VENDOR_NAME"`
	VendorUrl        string `ini:"VENDOR_URL"`
	Experiment       string `ini:"EXPERIMENT"`
	ExperimentUrl    string `ini:"EXPERIMENT_URL"`
	DefaultHostname  string `ini:"DEFAULT_HOSTNAME"`
	Architecture     string `ini:"ARCHITECTURE"`
	SysextLevel      string `ini:"SYSEXT_LEVEL"`
	SysextScope      string `ini:"SYSEXT_SCOPE"`
	ConfextLevel     string `ini:"CONFEXT_LEVEL"`
	ConfextScope     string `ini:"CONFEXT_SCOPE"`
	PortablePrefixes string `ini:"PORTABLE_PREFIXES"`
}

// ParseOsRelease parses the /etc/os-release file and returns an OsReleaseInfo struct.
func ParseOsRelease() (*OsReleaseInfo, error) {
	return ParseOsReleaseFile("/etc/os-release")
}

// ParseOsReleaseFile parses the given os-release file and returns an OsReleaseInfo struct.
func ParseOsReleaseFile(path string) (*OsReleaseInfo, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	return ParseOsReleaseString(data)
}

func ParseOsReleaseString(data []byte) (*OsReleaseInfo, error) {
	var info OsReleaseInfo

	lines := strings.Lines(string(data))

	values := make(map[string]string)

	for line := range lines {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}

		parts := strings.SplitN(line, "=", 2)
		if len(parts) != 2 {
			continue
		}

		key := strings.TrimSpace(parts[0])

		values[key] = trimQuotes(strings.TrimSpace(parts[1]))
	}

	// Reflectively set the fields of OsReleaseInfo
	infoValue := reflect.ValueOf(&info).Elem()
	infoTypeValue := infoValue.Type()

	for i := 0; i < infoTypeValue.NumField(); i++ {
		field := infoTypeValue.Field(i)
		iniTag := field.Tag.Get("ini")
		if iniTag == "" {
			continue
		}

		if val, ok := values[iniTag]; ok {
			infoValue.FieldByName(field.Name).SetString(val)
		}
	}

	return &info, nil
}

func trimQuotes(s string) string {
	if len(s) < 2 {
		return s
	}
	first := s[0]
	last := s[len(s)-1]
	if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
		return s[1 : len(s)-1]
	}
	return s
}
