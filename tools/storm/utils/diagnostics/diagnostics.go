package diagnostics

import (
	"archive/tar"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/klauspost/compress/zstd"
	"github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"

	stormsshclient "tridenttools/storm/utils/ssh/client"
	"tridenttools/storm/utils/trident"
	stormtrident "tridenttools/storm/utils/trident"
)

// VerifyBundleContents verifies that the diagnostics bundle contains all required files and validates report.json
func VerifyBundleContents(bundlePath string) error {
	file, err := os.Open(bundlePath)
	if err != nil {
		return fmt.Errorf("failed to open bundle file: %w", err)
	}
	defer file.Close()

	logrus.Infof("Extracting diagnostics bundle locally")
	zstdReader, err := zstd.NewReader(file)
	if err != nil {
		return fmt.Errorf("failed to create zstd decoder: %w", err)
	}
	defer zstdReader.Close()

	tarReader := tar.NewReader(zstdReader)
	foundFiles := make(map[string]bool)
	requiredFiles := []string{
		"datastore.sqlite",
		"report.json",
		"logs/trident-full.log",
		"logs/trident-metrics.jsonl",
	}
	hasHistoricalDir := false
	var reportJSON []byte

	for {
		header, err := tarReader.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return fmt.Errorf("failed to read tar entry: %w", err)
		}

		logrus.Debugf("  - %s (%d bytes)", header.Name, header.Size)

		// Check for required files
		for _, reqFile := range requiredFiles {
			if strings.HasSuffix(header.Name, reqFile) {
				foundFiles[reqFile] = true
			}
		}

		// Extract report.json content
		if strings.HasSuffix(header.Name, "report.json") {
			reportJSON, err = io.ReadAll(tarReader)
			if err != nil {
				return fmt.Errorf("failed to read report.json: %w", err)
			}
		}

		// Check for logs/historical directory
		if strings.Contains(header.Name, "logs/historical/") {
			hasHistoricalDir = true
		}
	}

	// Verify all required files are present
	var missingFiles []string
	for _, reqFile := range requiredFiles {
		if !foundFiles[reqFile] {
			missingFiles = append(missingFiles, reqFile)
		}
	}
	if !hasHistoricalDir {
		missingFiles = append(missingFiles, "logs/historical directory")
	}

	if len(missingFiles) > 0 {
		return fmt.Errorf("diagnostics bundle missing required files: %s", strings.Join(missingFiles, ", "))
	}

	// Validate report.json content
	var report map[string]interface{}
	if err := json.Unmarshal(reportJSON, &report); err != nil {
		return fmt.Errorf("failed to parse report.json: %w", err)
	}

	requiredReportFields := []string{"hostStatus", "hostDescription", "collectedFiles"}
	var missingReportFields []string
	for _, field := range requiredReportFields {
		if _, ok := report[field]; !ok {
			missingReportFields = append(missingReportFields, field)
		}
	}
	if len(missingReportFields) > 0 {
		return fmt.Errorf("report.json missing required fields: %s", strings.Join(missingReportFields, ", "))
	}

	return nil
}

// CheckDiagnostics runs trident diagnostics, copies the bundle locally, and verifies its contents
func CheckDiagnostics(client *ssh.Client, runtime trident.RuntimeType, envVars []string) error {
	// When running in container, write to /host/tmp so it persists on the host
	bundlePath := "/tmp/bundle"
	if runtime == trident.RuntimeTypeContainer {
		bundlePath = "/host/tmp/bundle"
	}

	logrus.Infof("Running trident diagnostics with output file: %s", bundlePath)
	out, err := stormtrident.InvokeTrident(runtime, client, envVars, fmt.Sprintf("diagnose --output %s --journal --selinux", bundlePath))
	if err != nil {
		return fmt.Errorf("failed to invoke trident diagnostics: %w", err)
	}
	if err := out.Check(); err != nil {
		logrus.Errorf("Trident diagnostics failed: %s", out.Report())
		return fmt.Errorf("trident diagnostics command failed: %w", err)
	}

	// Copy bundle to local system and verify its contents
	hostBundlePath := strings.TrimPrefix(bundlePath, "/host")
	localBundlePath := filepath.Join(os.TempDir(), "trident-diagnostics-bundle")
	if err := stormsshclient.CopyRemoteFileToLocal(client, hostBundlePath, localBundlePath); err != nil {
		return err
	}
	defer os.Remove(localBundlePath)

	if err := VerifyBundleContents(localBundlePath); err != nil {
		return err
	}

	logrus.Infof("Diagnostics bundle verified: all required files present")
	return nil
}
