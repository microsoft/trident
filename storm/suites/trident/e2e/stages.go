package trident

import (
	"fmt"
	"strings"
)

type TridentPipeline string

const (
	// Reserved for local development
	TridetPipelineLocal TridentPipeline = "local"
	TridentPipelinePr   TridentPipeline = "pr"
	TridentPipelineCi   TridentPipeline = "ci"
	TridentPipelinePre  TridentPipeline = "pre"
)

func (tp *TridentPipeline) UnmarshalText(text []byte) error {
	*tp = TridentPipeline(text)
	switch *tp {
	case TridetPipelineLocal:
	case TridentPipelinePr:
	case TridentPipelineCi:
	case TridentPipelinePre:
	default:
		return fmt.Errorf("invalid pipeline: %s", text)
	}

	return nil
}

type TridentMachine string

const (
	TridentMachineVm        TridentMachine = "vm"
	TridentMachineBareMetal TridentMachine = "bm"
)

func (tp *TridentMachine) UnmarshalText(text []byte) error {
	*tp = TridentMachine(text)
	switch *tp {
	case TridentMachineVm:
	case TridentMachineBareMetal:
	default:
		return fmt.Errorf("invalid machine: %s", text)
	}

	return nil
}

type TridentRuntime string

const (
	TridentRuntimeContainer TridentRuntime = "container"
	TridentRuntimeHost      TridentRuntime = "host"
)

func (tp *TridentRuntime) UnmarshalText(text []byte) error {
	*tp = TridentRuntime(text)
	switch *tp {
	case TridentRuntimeContainer:
	case TridentRuntimeHost:
	default:
		return fmt.Errorf("invalid runtime: %s", text)
	}

	return nil
}

type TridentE2EStagePath struct {
	Pipeline TridentPipeline
	Machine  TridentMachine
	Runtime  TridentRuntime
}

func NewTridentE2EStagePath(pipeline TridentPipeline, machine TridentMachine, runtime TridentRuntime) TridentE2EStagePath {
	return TridentE2EStagePath{
		Pipeline: pipeline,
		Machine:  machine,
		Runtime:  runtime,
	}
}

func (s TridentE2EStagePath) String() string {
	return fmt.Sprintf("e2e/%s/%s/%s", s.Pipeline, s.Machine, s.Runtime)
}

func TridentStagePathFromString(path string) (TridentE2EStagePath, error) {
	var tsp = TridentE2EStagePath{}
	err := tsp.UnmarshalText([]byte(path))
	return tsp, err
}

func (tsp *TridentE2EStagePath) UnmarshalText(text []byte) error {
	fmt.Println("Attempting to unmarshal text:", string(text))
	parts := strings.Split(string(text), "/")
	if len(parts) != 4 {
		return fmt.Errorf("trident E2E Stage paths should contain 4 components: %s", text)
	}

	if parts[0] != "e2e" {
		return fmt.Errorf("trident E2E Stage paths should begin with 'e2e/': %s", text)
	}

	if err := tsp.Pipeline.UnmarshalText([]byte(parts[1])); err != nil {
		return fmt.Errorf("invalid pipeline: %s", text)
	}
	if err := tsp.Machine.UnmarshalText([]byte(parts[2])); err != nil {
		return fmt.Errorf("invalid machine: %s", text)
	}
	if err := tsp.Runtime.UnmarshalText([]byte(parts[3])); err != nil {
		return fmt.Errorf("invalid runtime: %s", text)
	}

	return nil
}
