package tests

import (
	"fmt"
	"tridenttools/storm/servicing/utils/config"
)

func PublishSigImage(cfg config.ServicingConfig) error {
	if err := cfg.AzureConfig.PublishSigImage(cfg.TestConfig.ArtifactsDir, cfg.TestConfig.BuildId); err != nil {
		return fmt.Errorf("failed to publish Azure Shared Image Gallery image: %w", err)
	}
	return nil
}
