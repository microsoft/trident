package virtdeploy

import "testing"

func TestMatchesDomainName(t *testing.T) {
	ns := namespace("foo")
	for _, name := range []string{"foo-vm-0", "foo-vm-1", "foo-vm-999999"} {
		if !ns.matchesDomainName(name) {
			t.Errorf("canonical domain %q not selected", name)
		}
	}
	for _, name := range []string{
		"foobar-vm-0", "foo-bar-vm-0", "foo-vm-1-vm-0",
		"foo-network", "foo-pool", "foo-nvram-pool", "foo-vm-",
		"foo-vm-00", "foo-vm-01", "foo-vm-+1", "foo-vm--1", "foo-vm--0",
		"foo-vm- 1", "foo-vm-1 ", "foo-vm-1x", "foo-vm-999999999999999999999999",
	} {
		if ns.matchesDomainName(name) {
			t.Errorf("noncanonical or unrelated domain %q selected", name)
		}
	}
}

func TestPoolTargetDisambiguatesNamespace(t *testing.T) {
	for _, tt := range []struct {
		ns     namespace
		name   string
		target string
	}{
		{"foo", "foo-pool", "/var/lib/libvirt/foo"},
		{"foo", "foo-nvram-pool", "/var/lib/foo-nvram"},
		{"foo-nvram", "foo-nvram-pool", "/var/lib/libvirt/foo-nvram"},
		{"foo-nvram", "foo-nvram-nvram-pool", "/var/lib/foo-nvram-nvram"},
	} {
		target, selected := tt.ns.poolTarget(tt.name)
		if !selected || target != tt.target {
			t.Errorf("%s pool %s: got (%q, %v), want %q", tt.ns, tt.name, target, selected, tt.target)
		}
	}
	for _, name := range []string{"foobar-pool", "foo-bar-pool", "foo-vm-0-pool", "foo-network"} {
		if _, selected := namespace("foo").poolTarget(name); selected {
			t.Errorf("unrelated pool %q selected", name)
		}
	}
}
