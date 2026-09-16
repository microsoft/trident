package sysinspect

import "testing"

// util-linux emits lsblk's size as a quoted string on some versions and a bare
// number on others. Decoding it into an int64 fails on the string form, and
// because that aborts the whole json.Unmarshal it would take out every other
// field too, not just the size.
func TestParseLsblkAcceptsBothSizeEncodings(t *testing.T) {
	tests := []struct {
		name string
		json string
	}{
		{"numeric size", `{"blockdevices":[{"name":"sda","size":1234,"type":"disk"}]}`},
		{"quoted size", `{"blockdevices":[{"name":"sda","size":"1234","type":"disk"}]}`},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			out, err := ParseLsblk(tt.json)
			if err != nil {
				t.Fatalf("parse: %v", err)
			}
			if len(out.Blockdevices) != 1 {
				t.Fatalf("got %d devices, want 1", len(out.Blockdevices))
			}
			if got := out.Blockdevices[0].Name; got != "sda" {
				t.Errorf("name = %q, want sda", got)
			}
			if got := out.Blockdevices[0].Size.String(); got != "1234" {
				t.Errorf("size = %q, want 1234", got)
			}
		})
	}
}

// Partitions claims to return leaf devices. Stopping at the first level would
// return the partition that carries an encrypted/LVM child and omit the leaf.
func TestPartitionsDescendsNestedChildren(t *testing.T) {
	const nested = `{"blockdevices":[{"name":"sda","size":100,"children":[
		{"name":"sda1","size":10},
		{"name":"sda2","size":90,"children":[{"name":"luks-x","size":89}]}]}]}`

	out, err := ParseLsblk(nested)
	if err != nil {
		t.Fatalf("parse: %v", err)
	}

	var names []string
	for _, p := range out.Partitions() {
		names = append(names, p.Name)
	}
	if len(names) != 2 || names[0] != "sda1" || names[1] != "luks-x" {
		t.Errorf("Partitions() = %v, want [sda1 luks-x]", names)
	}

	// FindDevice must reach a device at any depth, since partition paths can
	// point at a nested node.
	if d, ok := out.FindDevice("luks-x"); !ok || d.Size.String() != "89" {
		t.Errorf("FindDevice(luks-x) = (%+v, %v)", d, ok)
	}
	if _, ok := out.FindDevice("nope"); ok {
		t.Error("FindDevice returned a device for an absent name")
	}
}
