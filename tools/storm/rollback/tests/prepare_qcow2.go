package tests

import (
	"fmt"
	"os"
	"os/exec"

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

const IMAGE_CUSTOMIZER_CMD_TEMPLATE = `docker run --rm \
	--privileged \
	-v ".:/repo:z" \
	-v "/dev:/dev" \
	%s \
		--log-level debug \
		--build-dir /build \
		--image-file /repo/%s \
		--output-image-file /repo/%s \
		--output-image-format qcow2 \
		--config-file /repo/%s
`

// Use Image Customizer to prepare the qcow2 image for rollback testing
// by injecting the extension v1.0.0
func PrepareQcow2(testConfig stormrollbackconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	// Find existing image file
	imageFile, err := stormfile.FindFile(testConfig.ArtifactsDir, "^trident-vm-.*-testimage.qcow2$")
	if err != nil {
		return fmt.Errorf("failed to find image file: %w", err)
	}
	logrus.Tracef("Found image file: %s", imageFile)

	// Find existing image file
	extensionFileName := "test-sysext-1.raw"
	extensionFile, err := stormfile.FindFile(testConfig.ArtifactsDir, fmt.Sprintf("^%s$", extensionFileName))
	if err != nil {
		return fmt.Errorf("failed to find extension file: %w", err)
	}
	logrus.Tracef("Found extension file: %s", extensionFile)
	// Create Image Customizer config file
	customizerConfigPath := fmt.Sprintf("%s/image-customizer-config.yaml", testConfig.OutputPath)
	customizerConfigContent := fmt.Sprintf(
		IMAGE_CUSTOMIZER_CONFIG_TEMPLATE,
		extensionFile,
		extensionFileName)
	if err := os.WriteFile(customizerConfigPath, []byte(customizerConfigContent), 0644); err != nil {
		return fmt.Errorf("failed to write image customizer config file: %w", err)
	}
	logrus.Tracef("Wrote image customizer config file: %s", customizerConfigPath)

	// Run Image Customizer
	imageCustmizerCommand := fmt.Sprintf(
		IMAGE_CUSTOMIZER_CMD_TEMPLATE,
		testConfig.ImageCustomizerImage,
		imageFile,
		"trident-vm-rollback-testimage.qcow2",
		customizerConfigPath)
	logrus.Tracef("Running Image Customizer command: %s", imageCustmizerCommand)
	if err := exec.Command(imageCustmizerCommand).Run(); err != nil {
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
