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
