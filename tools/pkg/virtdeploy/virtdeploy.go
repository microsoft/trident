package virtdeploy

import "fmt"

func CreateResources(config VirtDeployConfig) error {
	err := config.validate()
	if err != nil {
		return fmt.Errorf("failed to initialize config: %w", err)
	}

	resourceConfig, err := newVirtDeployResourceConfig(config)
	if err != nil {
		return fmt.Errorf("failed to create resource config: %w", err)
	}
	defer resourceConfig.close()

	err = resourceConfig.construct()
	if err != nil {
		return fmt.Errorf("failed to construct resources: %w", err)
	}

	return nil
}

func DeleteResources(namespace string) error {
	// Implementation would go here
	return nil
}
