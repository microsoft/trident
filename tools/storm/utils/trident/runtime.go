package trident

import "fmt"

type RuntimeType string

const (
	// RuntimeTypeHost indicates that the Trident service is running on the host.
	RuntimeTypeHost RuntimeType = "host"
	// RuntimeTypeContainer indicates that the Trident service is running in a container.
	RuntimeTypeContainer RuntimeType = "container"
	// RuntimeTypeNone indicates that the Trident service is not running.
	RuntimeTypeNone RuntimeType = "none"
)

type RuntimeCliSettings struct {
	TridentRuntimeType RuntimeType `arg:"" help:"Trident runtime type" enum:"host,container,none"`
}

func (tenv *RuntimeType) UnmarshalText(text []byte) error {
	*tenv = RuntimeType(text)
	switch *tenv {
	case RuntimeTypeHost, RuntimeTypeContainer, RuntimeTypeNone:
		return nil
	default:
		return fmt.Errorf("invalid Trident environment: %s", text)
	}
}

func (rt RuntimeType) ToString() string {
	return string(rt)
}

// HostPath returns the host path prefix based on the runtime type.
func (rt RuntimeType) HostPath() string {
	switch rt {
	case RuntimeTypeHost:
		return "/"
	case RuntimeTypeContainer:
		// In container runtime, the real root is expected to be mounted at /host.
		return "/host"
	default:
		return "/"
	}
}

func RuntimeTypes() []RuntimeType {
	return []RuntimeType{RuntimeTypeHost, RuntimeTypeContainer}
}
