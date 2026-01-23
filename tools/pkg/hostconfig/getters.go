package hostconfig

func (s *HostConfig) HasABUpdate() bool {
	return s.Container.Exists("storage", "abUpdate")
}
