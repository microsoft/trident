package proxies

import (
	"fmt"
	"os"
	"time"

	"gopkg.in/yaml.v3"
)

type Scenario struct {
	Steps []ScenarioStep `yaml:"steps"`
}

type ScenarioStep struct {
	Patch  *PatchStep  `yaml:"patch,omitempty"`
	Expect *ExpectStep `yaml:"expect,omitempty"`
}

type PatchStep struct {
	NodeUpdateID         string `yaml:"node-update-id,omitempty" json:"nodeUpdateId,omitempty"`
	OperationID          string `yaml:"operation-id,omitempty" json:"operationId,omitempty"`
	Operation            string `yaml:"operation,omitempty" json:"operation,omitempty"`
	TargetOSImageVersion string `yaml:"target-os-image-version,omitempty" json:"targetVersion,omitempty"`
	// Server overrides trident-aks-agent's configured Nebraska endpoint for
	// this request. The agent no longer has a config file at all (see
	// prepareVmForAksAgent), so any request that reaches Nebraska (stage,
	// finalize) must set this field.
	Server string `yaml:"server,omitempty" json:"server,omitempty"`
	// AppId overrides trident-aks-agent's configured Nebraska app_id for
	// this request, for the same reason Server does: there is no config
	// file to source it from.
	AppId string `yaml:"app-id,omitempty" json:"appId,omitempty"`
	// Track overrides trident-aks-agent's configured Nebraska track for
	// this request, for the same reason Server/AppId do: there is no
	// config file to source it from.
	Track string `yaml:"track,omitempty" json:"track,omitempty"`
}

type ExpectStep struct {
	OperationID   string        `yaml:"operation-id,omitempty" json:"operationId,omitempty"`
	Operation     string        `yaml:"operation,omitempty" json:"operation,omitempty"`
	Code          string        `yaml:"code" json:"code"`
	Timeout       time.Duration `yaml:"-" json:"timeoutSeconds"`
	TimeoutRaw    string        `yaml:"timeout,omitempty" json:"-"`
	ExpectTimeout bool          `yaml:"expect-timeout,omitempty" json:"expectTimeout,omitempty"`
}

type ScenarioReport struct {
	Passed bool         `json:"passed"`
	Steps  []StepReport `json:"steps"`
}

type StepReport struct {
	Index     int    `json:"index"`
	Kind      string `json:"kind"`
	Passed    bool   `json:"passed"`
	ElapsedMS int64  `json:"elapsedMs"`
	Message   string `json:"message"`
	Expected  any    `json:"expected,omitempty"`
	Actual    any    `json:"actual,omitempty"`
}

func LoadScenario(path string) (*Scenario, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read scenario %s: %w", path, err)
	}
	var scenario Scenario
	if err := yaml.Unmarshal(data, &scenario); err != nil {
		return nil, fmt.Errorf("failed to parse scenario yaml: %w", err)
	}
	if err := scenario.Validate(); err != nil {
		return nil, err
	}
	return &scenario, nil
}

func (s *Scenario) Validate() error {
	for index := range s.Steps {
		step := &s.Steps[index]
		kinds := 0
		if step.Patch != nil {
			kinds++
		}
		if step.Expect != nil {
			kinds++
		}
		if kinds != 1 {
			return fmt.Errorf("scenario step %d must set exactly one of patch/expect", index)
		}
		if step.Expect != nil {
			timeout := 60 * time.Second
			if step.Expect.TimeoutRaw != "" {
				var err error
				timeout, err = time.ParseDuration(step.Expect.TimeoutRaw)
				if err != nil {
					return fmt.Errorf("scenario step %d has invalid timeout %q: %w", index, step.Expect.TimeoutRaw, err)
				}
			}
			step.Expect.Timeout = timeout
		}
	}
	return nil
}
