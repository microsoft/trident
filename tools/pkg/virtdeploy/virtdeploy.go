package virtdeploy

import (
	"fmt"
	"runtime"
)

func CreateResources(config VirtDeployConfig) (*VirtDeployStatus, error) {
	// Initialize VM architecture to host architecture if not set
	for i := range config.VMs {
		if config.VMs[i].Arch == "" {
			config.VMs[i].Arch = runtime.GOARCH
		}
	}

	err := config.validate()
	if err != nil {
		return nil, fmt.Errorf("failed to initialize config: %w", err)
	}

	resourceConfig, err := newVirtDeployResourceConfig(config)
	if err != nil {
		return nil, fmt.Errorf("failed to create resource config: %w", err)
	}
	defer resourceConfig.close()

	status, err := resourceConfig.construct()
	if err != nil {
		return nil, fmt.Errorf("failed to construct resources: %w", err)
	}

	return status, nil
}

func DeleteResources(namespace string) error {
	err := cleanupNamespace(namespace)
	if err != nil {
		return fmt.Errorf("failed to clean up namespace: %w", err)
	}

	return nil
}
