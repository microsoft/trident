package env

import "fmt"

type TridentEnvironment string

const (
	// TridentEnvironmentHost indicates that the Trident service is running on the host.
	TridentEnvironmentHost TridentEnvironment = "host"
	// TridentEnvironmentContainer indicates that the Trident service is running in a container.
	TridentEnvironmentContainer TridentEnvironment = "container"
	// TridentEnvironmentNone indicates that the Trident service is not running.
	TridentEnvironmentNone TridentEnvironment = "none"
)

func (tenv *TridentEnvironment) UnmarshalText(text []byte) error {
	*tenv = TridentEnvironment(text)
	switch *tenv {
	case TridentEnvironmentHost, TridentEnvironmentContainer, TridentEnvironmentNone:
		return nil
	default:
		return fmt.Errorf("invalid Trident environment: %s", text)
	}
}

type EnvCliSettings struct {
	Env TridentEnvironment `arg:"" help:"Environment where Trident service is running" enum:"host,container,none"`
}

func (e TridentEnvironment) HostPath() string {
	switch e {
	case TridentEnvironmentHost:
		return "/"
	case TridentEnvironmentContainer:
		return "/host"
	default:
		return "/"
	}
}
