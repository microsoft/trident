package hostconfig

func (s *HostConfig) HasABUpdate() bool {
	return s.Container.Exists("storage", "abUpdate")
}

// HasRaid reports whether the Host Config declares software RAID.
func (s *HostConfig) HasRaid() bool {
	return s.Container.Exists("storage", "raid")
}

// HasEncryption reports whether the Host Config declares encryption.
func (s *HostConfig) HasEncryption() bool {
	return s.Container.Exists("storage", "encryption")
}

// HasVerity reports whether the Host Config declares verity.
func (s *HostConfig) HasVerity() bool {
	return s.Container.Exists("storage", "verity")
}

// HasRebuildableRaid reports whether the Host Config declares software RAID that
// supports rebuild testing: at least one storage.raid.software array must exist,
// and the config must not use usr-verity (verity rebuild is not yet supported —
// TODO(12277)). Mirrors the rebuild-raid helper's check-if-needed logic.
//
// The software list is checked rather than just the raid object because
// storage.raid.software is optional: a config carrying only sync_timeout is
// valid, and gating on the object alone would register the rebuild cases for it
// and then fail looking for a RAID member that was never configured.
func (s *HostConfig) HasRebuildableRaid() bool {
	if len(s.Container.S("storage", "raid", "software").Children()) == 0 {
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
