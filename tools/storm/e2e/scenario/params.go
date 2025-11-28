package scenario

type RuntimeType string

const (
	RuntimeTypeHost      RuntimeType = "host"
	RuntimeTypeContainer RuntimeType = "container"
)

func (rt RuntimeType) ToString() string {
	return string(rt)
}

type HardwareType string

const (
	HardwareTypeBM HardwareType = "bm"
	HardwareTypeVM HardwareType = "vm"
)

func (ht HardwareType) ToString() string {
	return string(ht)
}

func HardwareTypes() []HardwareType {
	return []HardwareType{HardwareTypeBM, HardwareTypeVM}
}

func RuntimeTypes() []RuntimeType {
	return []RuntimeType{RuntimeTypeHost, RuntimeTypeContainer}
}
