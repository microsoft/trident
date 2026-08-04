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
	Patch               *PatchStep  `yaml:"patch,omitempty"`
	Expect              *ExpectStep `yaml:"expect,omitempty"`
	AssertFailureReason string      `yaml:"assert-failure-reason,omitempty"`
}

type PatchStep struct {
	Request              string `yaml:"request,omitempty" json:"request,omitempty"`
	RequestID            string `yaml:"request-id,omitempty" json:"request-id,omitempty"`
	TargetOSImageVersion string `yaml:"target-os-image-version,omitempty" json:"target-os-image-version,omitempty"`
}

type ExpectStep struct {
	State             string        `yaml:"state" json:"state"`
	ObservedRequestID string        `yaml:"observed-request-id,omitempty" json:"observedRequestId,omitempty"`
	Timeout           time.Duration `yaml:"-" json:"timeoutSeconds"`
	TimeoutRaw        string        `yaml:"timeout,omitempty" json:"-"`
	ExpectTimeout     bool          `yaml:"expect-timeout,omitempty" json:"expectTimeout,omitempty"`
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
		if step.AssertFailureReason != "" {
			kinds++
		}
		if kinds != 1 {
			return fmt.Errorf("scenario step %d must set exactly one of patch/expect/assert-failure-reason", index)
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

func (p *PatchStep) Labels() map[string]string {
	labels := map[string]string{}
	if p.Request != "" {
		labels[RequestLabel] = p.Request
	}
	if p.RequestID != "" {
		labels[RequestIDLabel] = p.RequestID
	}
	if p.TargetOSImageVersion != "" {
		labels[TargetVersionLabel] = p.TargetOSImageVersion
	}
	return labels
}
