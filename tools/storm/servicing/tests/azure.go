package tests

import (
	"fmt"
	stormsvcconfig "tridenttools/storm/servicing/utils/config"
	stormvmconfig "tridenttools/storm/utils/vm/config"
)

func PublishSigImage(testConfig stormsvcconfig.TestConfig, vmConfig stormvmconfig.AllVMConfig) error {
	if err := vmConfig.AzureConfig.PublishSigImage(testConfig.ArtifactsDir); err != nil {
		return fmt.Errorf("failed to publish Azure Shared Image Gallery image: %w", err)
	}
	return nil
}
