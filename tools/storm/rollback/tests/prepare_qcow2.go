package tests

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	stormrollbackconfig "tridenttools/storm/rollback/utils/config"
	stormfile "tridenttools/storm/utils/file"
	stormvmconfig "tridenttools/storm/utils/vm/config"

	"github.com/sirupsen/logrus"
)

const IMAGE_CUSTOMIZER_CONFIG_TEMPLATE = `# config.yaml
previewFeatures:
- reinitialize-verity
os:
  additionalFiles:
  - source: %s
    destination: /var/lib/extensions/%s

  services:
    enable:
    - systemd-sysext
`

// Use Image Customizer to prepare the qcow2 image for rollback testing
// by injecting the extension v1.0.0
func PrepareQcow2(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if testConfig.SkipExtensionTesting {
		logrus.Infof("Skipping qcow2 preparation since SkipExtensionTesting is set")
		return nil
	}
	// Find existing image file
	imageFilePath, err := stormfile.FindFile(testConfig.ArtifactsDir, vmConfig.QemuConfig.Qcow2Pattern)
	if err != nil {
		return fmt.Errorf("failed to find image file: %w", err)
	}
	logrus.Tracef("Found image file: %s", imageFilePath)
	imageFileFolder := filepath.Dir(imageFilePath)
	imageFileFolder, err = filepath.Abs(imageFileFolder)
	if err != nil {
		return fmt.Errorf("failed to get absolute path of image file folder: %w", err)
	}
	imageFileName := filepath.Base(imageFilePath)

	// Find existing image file
	extensionFileName := fmt.Sprintf("%s-1.raw", testConfig.ExtensionName)
	extensionFile, err := stormfile.FindFile(testConfig.ArtifactsDir, fmt.Sprintf("^%s$", extensionFileName))
	if err != nil {
		return fmt.Errorf("failed to find extension file: %w", err)
	}
	logrus.Tracef("Found extension file: %s", extensionFile)

	// Create Image Customizer config
	customizerConfigFile := "image-customizer-config.yaml"
	customizerConfigPath := filepath.Join(testConfig.ArtifactsDir, customizerConfigFile)
	customizerConfigContent := fmt.Sprintf(
		IMAGE_CUSTOMIZER_CONFIG_TEMPLATE,
		filepath.Join("/artifacts", extensionFileName),
		extensionFileName,
	)
	logrus.Tracef("Creating image customizer config file: %s", customizerConfigPath)
	logrus.Tracef("Image customizer config content:\n%s", customizerConfigContent)
	if err := os.WriteFile(customizerConfigPath, []byte(customizerConfigContent), 0644); err != nil {
		return fmt.Errorf("failed to write image customizer config file: %w", err)
	}
	logrus.Tracef("Wrote image customizer config file: %s", customizerConfigPath)

	// Pull Image Customizer image
	pullArgs := []string{"pull", testConfig.ImageCustomizerImage}
	logrus.Tracef("Pulling Image Customizer image: %v", pullArgs)
	pullOutput, err := exec.Command("docker", pullArgs...).CombinedOutput()
	logrus.Tracef("Pull image customizer (%v):\n%s", err, string(pullOutput))
	if err != nil {
		return fmt.Errorf("failed to pull image customizer image: %w", err)
	}
	logrus.Tracef("Pulled Image Customizer image: %s", testConfig.ImageCustomizerImage)

	// Run Image Customizer
	customizedImageFileName := "tmp-adjusted-rollback-testimage.qcow2"
	icRunArgs := []string{
		"run",
		"--pull=never",
		"--rm",
		"--privileged",
		"-v", fmt.Sprintf("%s:/input-image:z", imageFileFolder),
		"-v", fmt.Sprintf("%s:/artifacts", testConfig.ArtifactsDir),
		"-v", "/dev:/dev",
		testConfig.ImageCustomizerImage,
		"--log-level", "debug",
		"--build-dir", "/build",
		"--image-file", fmt.Sprintf("/input-image/%s", imageFileName),
		"--output-image-file", fmt.Sprintf("/artifacts/%s", customizedImageFileName),
		"--output-image-format", "qcow2",
		"--config-file", fmt.Sprintf("/artifacts/%s", customizerConfigFile),
	}
	logrus.Tracef("Running Image Customizer command: %v", icRunArgs)
	icRunOutput, err := exec.Command("docker", icRunArgs...).CombinedOutput()
	logrus.Tracef("Run image customizer (%v):\n%s", err, string(icRunOutput))
	if err != nil {
		return fmt.Errorf("failed to run image customizer: %w", err)
	}
	logrus.Tracef("Image Customizer completed successfully")

	// Remove original image file
	if err := os.Remove(imageFilePath); err != nil {
		return fmt.Errorf("failed to remove original image file: %w", err)
	}
	logrus.Tracef("Removed original image file: %s", imageFilePath)

	// Move new image file to original file location
	customizedImageFilePath := filepath.Join(testConfig.ArtifactsDir, customizedImageFileName)
	if err := exec.Command("mv", customizedImageFilePath, imageFilePath).Run(); err != nil {
		return fmt.Errorf("failed to move new image file to original location: %w", err)
	}
	logrus.Tracef("Moved new image file to original location: %s", imageFilePath)

	return nil
}
