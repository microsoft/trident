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
	// Find existing image file
	imageFile, err := stormfile.FindFile(testConfig.ArtifactsDir, vmConfig.QemuConfig.Qcow2Pattern)
	if err != nil {
		return fmt.Errorf("failed to find image file: %w", err)
	}
	logrus.Tracef("Found image file: %s", imageFile)
	imageFileFolder := filepath.Dir(imageFile)
	imageFileName := filepath.Base(imageFile)

	// Find existing image file
	extensionFileName := "test-sysext-1.raw"
	extensionFile, err := stormfile.FindFile(testConfig.ArtifactsDir, fmt.Sprintf("^%s$", extensionFileName))
	if err != nil {
		return fmt.Errorf("failed to find extension file: %w", err)
	}
	logrus.Tracef("Found extension file: %s", extensionFile)

	// Create Image Customizer config file
	customizerConfigPath := filepath.Join(testConfig.ArtifactsDir, "image-customizer-config.yaml")
	customizerConfigContent := fmt.Sprintf(
		IMAGE_CUSTOMIZER_CONFIG_TEMPLATE,
		extensionFile,
		extensionFileName)
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
	icRunArgs := []string{
		"run",
		"--pull=never",
		"--rm",
		"--privileged",
		"-v", fmt.Sprintf("%s:/input-image:z", imageFileFolder),
		"-v", ".:/repo:z",
		"-v", "/dev:/dev",
		testConfig.ImageCustomizerImage,
		"--log-level", "debug",
		"--build-dir", "/build",
		"--image-file", fmt.Sprintf("/input-image/%s", imageFileName),
		"--output-image-file", "/repo/trident-vm-rollback-testimage.qcow2",
		"--output-image-format", "qcow2",
		"--config-file", fmt.Sprintf("/repo/%s", customizerConfigPath),
	}
	logrus.Tracef("Running Image Customizer command: %v", icRunArgs)
	icRunOutput, err := exec.Command("docker", icRunArgs...).CombinedOutput()
	logrus.Tracef("Run image customizer (%v):\n%s", err, string(icRunOutput))
	if err != nil {
		return fmt.Errorf("failed to run image customizer: %w", err)
	}
	logrus.Tracef("Image Customizer completed successfully")

	// Remove original image file
	if err := os.Remove(imageFile); err != nil {
		return fmt.Errorf("failed to remove original image file: %w", err)
	}
	logrus.Tracef("Removed original image file: %s", imageFile)

	// Move new image file to original file location
	if err := exec.Command("mv", "trident-vm-rollback-testimage.qcow2", imageFile).Run(); err != nil {
		return fmt.Errorf("failed to move new image file to original location: %w", err)
	}
	logrus.Tracef("Moved new image file to original location: %s", imageFile)

	return nil
}
