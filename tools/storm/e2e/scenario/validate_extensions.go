package scenario

import (
	"fmt"
	"path/filepath"
	"strings"

	"github.com/microsoft/storm"
	"github.com/sirupsen/logrus"

	"tridenttools/storm/utils/trident"
)

// validateExtensions validates systemd-sysext and confext extensions on the
// remote host. Converted from extensions_test.py test_extensions.
//
// It validates:
//   - systemd-sysext/confext status returns valid JSON
//   - Each configured extension path exists on the target OS
//   - Each configured extension name appears in the active extension list
func (s *TridentE2EScenario) validateExtensions(tc storm.TestCase) error {
	if err := s.populateSshClient(tc.Context()); err != nil {
		return fmt.Errorf("failed to populate SSH client: %w", err)
	}

	// Get host status via trident get
	tridentOut, err := trident.InvokeTrident(s.runtime, s.sshClient, nil, "get")
	if err != nil {
		return fmt.Errorf("failed to run trident get: %w", err)
	}

	if tridentOut.Status != 0 {
		return fmt.Errorf("trident get failed with status %d: %s",
			tridentOut.Status, tridentOut.Stderr)
	}

	hostStatus, err := ParseTridentGetOutput(tridentOut.Stdout)
	if err != nil {
		return fmt.Errorf("failed to parse trident get output: %w", err)
	}

	spec, _ := hostStatus["spec"].(map[interface{}]interface{})
	if spec == nil {
		tc.Fail("no spec found in host status")
		return nil
	}

	osConfig, _ := spec["os"].(map[interface{}]interface{})
	if osConfig == nil {
		tc.Fail("no os config found in host status spec")
		return nil
	}

	// Validate each extension type: sysext and confext
	for _, extType := range []string{"sysext", "confext"} {
		configKey := extType + "s"
		extConfigRaw, ok := osConfig[configKey]
		if !ok {
			continue
		}

		extConfigList, ok := extConfigRaw.([]interface{})
		if !ok || len(extConfigList) == 0 {
			continue
		}

		logrus.Infof("Validating %s extensions (%d configured)", extType, len(extConfigList))

		// Run systemd-sysext/confext status --json=pretty
		statusOut, err := sudoCommand(s.sshClient,
			fmt.Sprintf("systemd-%s status --json=pretty --no-pager", extType))
		if err != nil {
			tc.Fail(fmt.Sprintf("failed to run 'systemd-%s status': %v", extType, err))
			continue
		}

		hierarchies, err := ParseSysextStatus(statusOut)
		if err != nil {
			tc.Fail(fmt.Sprintf("failed to parse 'systemd-%s status' JSON: %v", extType, err))
			continue
		}

		activeExts := AllActiveExtensions(hierarchies)

		for _, extRaw := range extConfigList {
			ext, ok := extRaw.(map[interface{}]interface{})
			if !ok {
				continue
			}

			extPath, _ := ext["path"].(string)
			if extPath == "" {
				continue
			}

			// Verify that the path exists on the target OS
			_, err := sudoCommand(s.sshClient, fmt.Sprintf("test -e %s", extPath))
			if err != nil {
				tc.Fail(fmt.Sprintf("%s path does not exist: %s", extType, extPath))
				continue
			}

			// Extract extension name from path (equivalent to Python's Path.stem)
			extName := strings.TrimSuffix(filepath.Base(extPath), filepath.Ext(extPath))

			if !containsString(activeExts, extName) {
				tc.Fail(fmt.Sprintf("%s '%s' not found in 'systemd-%s status'",
					extType, extName, extType))
				continue
			}

			logrus.Infof("Extension validated: %s %s (%s)", extType, extName, extPath)
		}
	}

	logrus.Info("Extensions validation passed")
	return nil
}

// containsString checks if a string slice contains a given value.
func containsString(slice []string, val string) bool {
	for _, s := range slice {
		if s == val {
			return true
		}
	}
	return false
}
