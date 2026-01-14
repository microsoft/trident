package hostconfig

import (
	"fmt"

	"github.com/Jeffail/gabs/v2"
	"gopkg.in/yaml.v3"
)

// HostConfig represents the configuration for a host. It uses a gabs.Container
// to manage the underlying data structure.
type HostConfig struct {
	*gabs.Container
}

// NewHostConfigFromContainer creates a new HostConfig from an existing gabs.Container.
func NewHostConfigFromContainer(container *gabs.Container) HostConfig {
	return HostConfig{
		Container: container,
	}
}

// NewHostConfigFromInterface creates a new HostConfig from a generic interface{}.
func NewHostConfigFromInterface(data interface{}) HostConfig {
	return HostConfig{
		Container: gabs.Wrap(data),
	}
}

// NewHostConfigFromYaml creates a new HostConfig from a YAML byte slice.
func NewHostConfigFromYaml(yamlData []byte) (HostConfig, error) {
	var data map[string]any
	err := yaml.Unmarshal(yamlData, &data)
	if err != nil {
		return HostConfig{}, fmt.Errorf("failed to unmarshal YAML data: %w", err)
	}

	return HostConfig{
		Container: gabs.Wrap(data),
	}, nil
}

// GetContainer returns the underlying gabs.Container of the HostConfig.
func (hc *HostConfig) GetContainer() *gabs.Container {
	return hc.Container
}

// Data returns the underlying data of the HostConfig as an interface{}.
func (hc *HostConfig) ToInterface() interface{} {
	return hc.Container.Data()
}

// ToYaml serializes the HostConfig to a YAML byte slice.
func (hc *HostConfig) ToYaml() ([]byte, error) {
	data, err := yaml.Marshal(hc.Data())
	if err != nil {
		return nil, fmt.Errorf("failed to marshal HostConfig to YAML: %w", err)
	}

	return data, nil
}

// Clone creates a deep copy of the HostConfig.
func (s *HostConfig) Clone() (HostConfig, error) {
	// Unfortunately there is no direct way to clone an interface{}/any type, in
	// Go, so the easiest way is to serialize to YAML and deserialize back. This
	// is by no means the most efficient way, but it works for our purposes.
	yamlData, err := s.ToYaml()
	if err != nil {
		return HostConfig{}, err
	}

	copy, err := NewHostConfigFromYaml(yamlData)
	if err != nil {
		return HostConfig{}, err
	}

	return copy, nil
}
