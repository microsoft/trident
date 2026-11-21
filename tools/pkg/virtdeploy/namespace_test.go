package virtdeploy

import "testing"

func TestNamespaceOwnership(t *testing.T) {
	ns := namespace("foo")
	for _, name := range []string{"foo-network", "foo-pool", "foo-nvram-pool", "foo-vm-0", "foo-vm-12"} {
		if !ns.isInNamespace(name) {
			t.Errorf("namespace should own %q", name)
		}
	}
	for _, name := range []string{"foo-other-network", "foo-other-vm-0", "foo-vm-other", "foo-vm--1", "foo-vm-0-backup", "foo-backup-pool", "foobar-vm-0"} {
		if ns.isInNamespace(name) {
			t.Errorf("namespace must not own %q", name)
		}
	}
}
