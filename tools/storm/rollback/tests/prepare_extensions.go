package tests

import (
	"fmt"
	"os"
	"path/filepath"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	buildextensions "tridenttools/storm/scripts/build_extension_images"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

// sysextCloneCount is the number of test sysext images the rollback flow needs.
// The extension update test walks through the clones one version at a time.
const sysextCloneCount = 3

// PrepareExtensions makes sure the test sysext images the rollback flow needs
// exist in the artifacts directory.
//
// These used to be produced by a pipeline step that ran the
// build-extension-images script and moved the results into place, which meant a
// local run silently failed later with "failed to find extension file". Owning
// it here keeps the scenario self-sufficient on a dev box, and skips the work
// entirely for flavors that do not exercise extensions.
func PrepareExtensions(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if testConfig.SkipExtensionTesting {
		logrus.Infof("Skipping extension image preparation since SkipExtensionTesting is set")
		return nil
	}

	if err := os.MkdirAll(testConfig.ArtifactsDir, 0755); err != nil {
		return fmt.Errorf("failed to create artifacts directory %s: %w", testConfig.ArtifactsDir, err)
	}

	if missing, err := missingSysexts(testConfig); err != nil {
		return err
	} else if len(missing) == 0 {
		logrus.Infof("All %d test sysext images already present in %s", sysextCloneCount, testConfig.ArtifactsDir)
		return nil
	} else {
		logrus.Infof("Building %d test sysext images in %s (missing: %v)", sysextCloneCount, testConfig.ArtifactsDir, missing)
	}

	if err := buildextensions.BuildSysextImages(testConfig.ArtifactsDir, sysextCloneCount); err != nil {
		return fmt.Errorf("failed to build test sysext images: %w", err)
	}

	// A partial build would otherwise surface much later as a confusing
	// "failed to find extension file" during qcow2 preparation.
	if missing, err := missingSysexts(testConfig); err != nil {
		return err
	} else if len(missing) > 0 {
		return fmt.Errorf("test sysext images still missing after build: %v", missing)
	}

	return nil
}

// missingSysexts returns the sysext image paths that are not yet present.
func missingSysexts(testConfig stormrollbackconfig.TestConfig) ([]string, error) {
	var missing []string
	for i := 1; i <= sysextCloneCount; i++ {
		path := filepath.Join(testConfig.ArtifactsDir, fmt.Sprintf("%s-%d.raw", testConfig.ExtensionName, i))
		switch _, err := os.Stat(path); {
		case err == nil:
		case os.IsNotExist(err):
			missing = append(missing, path)
		default:
			return nil, fmt.Errorf("failed to stat %s: %w", path, err)
		}
	}
	return missing, nil
}
