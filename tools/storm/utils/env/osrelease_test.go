package env

import (
	"os"
	"path/filepath"
	"testing"
)

func TestParseOsReleaseString(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected OsReleaseInfo
		wantErr  bool
	}{
		{
			name: "basic os-release",
			input: `NAME="Test Linux"
ID=testlinux
VERSION_ID=1.0
PRETTY_NAME="Test Linux 1.0"`,
			expected: OsReleaseInfo{
				Name:       "Test Linux",
				Id:         "testlinux",
				VersionId:  "1.0",
				PrettyName: "Test Linux 1.0",
			},
			wantErr: false,
		},
		{
			name: "os-release with single quotes",
			input: `NAME='Ubuntu'
VERSION='22.04 LTS (Jammy Jellyfish)'
ID=ubuntu
VERSION_ID='22.04'`,
			expected: OsReleaseInfo{
				Name:      "Ubuntu",
				Version:   "22.04 LTS (Jammy Jellyfish)",
				Id:        "ubuntu",
				VersionId: "22.04",
			},
			wantErr: false,
		},
		{
			name: "os-release with unquoted values",
			input: `NAME=Fedora
VERSION=38
ID=fedora
VERSION_ID=38`,
			expected: OsReleaseInfo{
				Name:      "Fedora",
				Version:   "38",
				Id:        "fedora",
				VersionId: "38",
			},
			wantErr: false,
		},
		{
			name: "os-release with comments and empty lines",
			input: `# This is a comment
NAME="Test Linux"

# Another comment
ID=testlinux
VERSION_ID=1.0

`,
			expected: OsReleaseInfo{
				Name:      "Test Linux",
				Id:        "testlinux",
				VersionId: "1.0",
			},
			wantErr: false,
		},
		{
			name: "complete os-release with all fields",
			input: `NAME="Azure Linux"
ID=azurelinux
ID_LIKE="rhel fedora"
VERSION="3.0"
VERSION_ID=3.0
VERSION_CODENAME=mariner
PRETTY_NAME="Azure Linux 3.0"
CPE_NAME="cpe:/o:microsoft:azurelinux:3.0"
HOME_URL="https://aka.ms/azurelinux"
SUPPORT_URL="https://aka.ms/azurelinux/support"
BUG_REPORT_URL="https://github.com/microsoft/azurelinux"
ANSI_COLOR="1;34"
VENDOR_NAME="Microsoft"
VENDOR_URL="https://www.microsoft.com"`,
			expected: OsReleaseInfo{
				Name:            "Azure Linux",
				Id:              "azurelinux",
				IdLike:          "rhel fedora",
				Version:         "3.0",
				VersionId:       "3.0",
				VersionCodename: "mariner",
				PrettyName:      "Azure Linux 3.0",
				CpeName:         "cpe:/o:microsoft:azurelinux:3.0",
				HomeUrl:         "https://aka.ms/azurelinux",
				SupportUrl:      "https://aka.ms/azurelinux/support",
				BugReportUrl:    "https://github.com/microsoft/azurelinux",
				AnsiColor:       "1;34",
				VendorName:      "Microsoft",
				VendorUrl:       "https://www.microsoft.com",
			},
			wantErr: false,
		},
		{
			name: "os-release with systemd extension fields",
			input: `NAME="Test Sysext"
ID=testsysext
SYSEXT_LEVEL=1.0
SYSEXT_SCOPE=system
CONFEXT_LEVEL=1.0
CONFEXT_SCOPE=system
PORTABLE_PREFIXES=app1 app2`,
			expected: OsReleaseInfo{
				Name:             "Test Sysext",
				Id:               "testsysext",
				SysextLevel:      "1.0",
				SysextScope:      "system",
				ConfextLevel:     "1.0",
				ConfextScope:     "system",
				PortablePrefixes: "app1 app2",
			},
			wantErr: false,
		},
		{
			name: "os-release with malformed lines",
			input: `NAME="Test Linux"
INVALID_LINE_WITHOUT_EQUALS
ID=testlinux
=VALUE_WITHOUT_KEY
VERSION_ID=1.0`,
			expected: OsReleaseInfo{
				Name:      "Test Linux",
				Id:        "testlinux",
				VersionId: "1.0",
			},
			wantErr: false,
		},
		{
			name:     "empty os-release",
			input:    ``,
			expected: OsReleaseInfo{},
			wantErr:  false,
		},
		{
			name: "os-release with whitespace",
			input: `  NAME  =  "Test Linux"  
  ID  =  testlinux  
  VERSION_ID  =  1.0  `,
			expected: OsReleaseInfo{
				Name:      "Test Linux",
				Id:        "testlinux",
				VersionId: "1.0",
			},
			wantErr: false,
		},
		{
			name: "os-release with special characters in values",
			input: `NAME="Test Linux (LTS)"
VERSION="1.0 \"Stable\""
HOME_URL="https://test.com?param=value&other=123"`,
			expected: OsReleaseInfo{
				Name:    "Test Linux (LTS)",
				Version: "1.0 \\\"Stable\\\"",
				HomeUrl: "https://test.com?param=value&other=123",
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := ParseOsReleaseString([]byte(tt.input))
			if (err != nil) != tt.wantErr {
				t.Errorf("ParseOsReleaseString() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if err == nil {
				if got.Name != tt.expected.Name {
					t.Errorf("Name = %v, want %v", got.Name, tt.expected.Name)
				}
				if got.Id != tt.expected.Id {
					t.Errorf("Id = %v, want %v", got.Id, tt.expected.Id)
				}
				if got.IdLike != tt.expected.IdLike {
					t.Errorf("IdLike = %v, want %v", got.IdLike, tt.expected.IdLike)
				}
				if got.Version != tt.expected.Version {
					t.Errorf("Version = %v, want %v", got.Version, tt.expected.Version)
				}
				if got.VersionId != tt.expected.VersionId {
					t.Errorf("VersionId = %v, want %v", got.VersionId, tt.expected.VersionId)
				}
				if got.VersionCodename != tt.expected.VersionCodename {
					t.Errorf("VersionCodename = %v, want %v", got.VersionCodename, tt.expected.VersionCodename)
				}
				if got.PrettyName != tt.expected.PrettyName {
					t.Errorf("PrettyName = %v, want %v", got.PrettyName, tt.expected.PrettyName)
				}
				if got.CpeName != tt.expected.CpeName {
					t.Errorf("CpeName = %v, want %v", got.CpeName, tt.expected.CpeName)
				}
				if got.HomeUrl != tt.expected.HomeUrl {
					t.Errorf("HomeUrl = %v, want %v", got.HomeUrl, tt.expected.HomeUrl)
				}
				if got.SupportUrl != tt.expected.SupportUrl {
					t.Errorf("SupportUrl = %v, want %v", got.SupportUrl, tt.expected.SupportUrl)
				}
				if got.BugReportUrl != tt.expected.BugReportUrl {
					t.Errorf("BugReportUrl = %v, want %v", got.BugReportUrl, tt.expected.BugReportUrl)
				}
				if got.AnsiColor != tt.expected.AnsiColor {
					t.Errorf("AnsiColor = %v, want %v", got.AnsiColor, tt.expected.AnsiColor)
				}
				if got.VendorName != tt.expected.VendorName {
					t.Errorf("VendorName = %v, want %v", got.VendorName, tt.expected.VendorName)
				}
				if got.VendorUrl != tt.expected.VendorUrl {
					t.Errorf("VendorUrl = %v, want %v", got.VendorUrl, tt.expected.VendorUrl)
				}
				if got.SysextLevel != tt.expected.SysextLevel {
					t.Errorf("SysextLevel = %v, want %v", got.SysextLevel, tt.expected.SysextLevel)
				}
				if got.SysextScope != tt.expected.SysextScope {
					t.Errorf("SysextScope = %v, want %v", got.SysextScope, tt.expected.SysextScope)
				}
				if got.ConfextLevel != tt.expected.ConfextLevel {
					t.Errorf("ConfextLevel = %v, want %v", got.ConfextLevel, tt.expected.ConfextLevel)
				}
				if got.ConfextScope != tt.expected.ConfextScope {
					t.Errorf("ConfextScope = %v, want %v", got.ConfextScope, tt.expected.ConfextScope)
				}
				if got.PortablePrefixes != tt.expected.PortablePrefixes {
					t.Errorf("PortablePrefixes = %v, want %v", got.PortablePrefixes, tt.expected.PortablePrefixes)
				}
			}
		})
	}
}

func TestParseOsReleaseFile(t *testing.T) {
	// Create a temporary directory for test files
	tmpDir := t.TempDir()

	tests := []struct {
		name        string
		fileContent string
		fileName    string
		expected    OsReleaseInfo
		wantErr     bool
	}{
		{
			name:     "valid os-release file",
			fileName: "os-release",
			fileContent: `NAME="Test Linux"
ID=testlinux
VERSION_ID=1.0`,
			expected: OsReleaseInfo{
				Name:      "Test Linux",
				Id:        "testlinux",
				VersionId: "1.0",
			},
			wantErr: false,
		},
		{
			name:     "non-existent file",
			fileName: "nonexistent",
			wantErr:  true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var filePath string
			if tt.fileContent != "" {
				filePath = filepath.Join(tmpDir, tt.fileName)
				err := os.WriteFile(filePath, []byte(tt.fileContent), 0644)
				if err != nil {
					t.Fatalf("Failed to create test file: %v", err)
				}
			} else {
				filePath = filepath.Join(tmpDir, tt.fileName)
			}

			got, err := ParseOsReleaseFile(filePath)
			if (err != nil) != tt.wantErr {
				t.Errorf("ParseOsReleaseFile() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if err == nil {
				if got.Name != tt.expected.Name {
					t.Errorf("Name = %v, want %v", got.Name, tt.expected.Name)
				}
				if got.Id != tt.expected.Id {
					t.Errorf("Id = %v, want %v", got.Id, tt.expected.Id)
				}
				if got.VersionId != tt.expected.VersionId {
					t.Errorf("VersionId = %v, want %v", got.VersionId, tt.expected.VersionId)
				}
			}
		})
	}
}

func TestTrimQuotes(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "double quoted string",
			input:    `"hello world"`,
			expected: "hello world",
		},
		{
			name:     "single quoted string",
			input:    "'hello world'",
			expected: "hello world",
		},
		{
			name:     "unquoted string",
			input:    "hello world",
			expected: "hello world",
		},
		{
			name:     "mismatched quotes",
			input:    `"hello world'`,
			expected: `"hello world'`,
		},
		{
			name:     "empty string",
			input:    "",
			expected: "",
		},
		{
			name:     "single character",
			input:    "a",
			expected: "a",
		},
		{
			name:     "just double quotes",
			input:    `""`,
			expected: "",
		},
		{
			name:     "just single quotes",
			input:    "''",
			expected: "",
		},
		{
			name:     "string with quotes inside",
			input:    `"hello \"world\""`,
			expected: `hello \"world\"`,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := trimQuotes(tt.input)
			if got != tt.expected {
				t.Errorf("trimQuotes() = %v, want %v", got, tt.expected)
			}
		})
	}
}
