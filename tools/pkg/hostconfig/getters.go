package hostconfig

func (s *HostConfig) HasABUpdate() bool {
	return s.Container.Exists("storage", "abUpdate")
}

// HasRebuildableRaid reports whether the Host Config declares software RAID that
// supports rebuild testing: a storage.raid section must exist, and the config
// must not use usr-verity (verity rebuild is not yet supported — TODO(12277)).
// Mirrors the rebuild-raid helper's check-if-needed logic.
func (s *HostConfig) HasRebuildableRaid() bool {
	if !s.Container.Exists("storage", "raid") {
		return false
	}
	return !s.hasUsrVerity()
}

// hasUsrVerity reports whether the Host Config declares a verity device named
// "usr".
func (s *HostConfig) hasUsrVerity() bool {
	for _, verity := range s.Container.S("storage", "verity").Children() {
		if name, ok := verity.S("name").Data().(string); ok && name == "usr" {
			return true
		}
	}
	return false
}
