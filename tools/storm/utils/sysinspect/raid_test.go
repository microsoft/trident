package sysinspect

import "testing"

// parseRaidName maps a kernel md device to the friendly name mdadm symlinks it
// under. The A/B RAID assertion compares that name to the configured array, so
// a wrong answer here validates the wrong device.
func TestParseRaidNameResolvesTheMatchingArray(t *testing.T) {
	const listing = `total 0
lrwxrwxrwx 1 root root 8 Sep 17 20:00 root-a -> ../md127
lrwxrwxrwx 1 root root 8 Sep 17 20:00 root-b -> ../md126
`
	for _, tt := range []struct {
		device string
		want   string
		found  bool
	}{
		{"/dev/md127", "/dev/md/root-a", true},
		{"md126", "/dev/md/root-b", true},
		{"/dev/md125", "", false},
	} {
		got, found := parseRaidName(listing, tt.device)
		if found != tt.found || got != tt.want {
			t.Errorf("parseRaidName(%q) = (%q, %v), want (%q, %v)", tt.device, got, found, tt.want, tt.found)
		}
	}
}

// The sentinel exists because `ls` exits 2 both when /dev/md is missing and
// when it cannot be read. Only the first is a legitimate "no RAID here";
// collapsing the second into it would turn an inspection failure into a
// confident negative and silently validate the wrong device set.
func TestNoMdDirectorySentinelIsDistinctFromAListing(t *testing.T) {
	if got, found := parseRaidName(noMdDirectorySentinel, "/dev/md127"); found || got != "" {
		t.Errorf("sentinel parsed as a listing: (%q, %v)", got, found)
	}
	// An empty /dev/md is a real listing with no arrays, which is not an error.
	if got, found := parseRaidName("total 0\n", "/dev/md127"); found || got != "" {
		t.Errorf("empty listing: (%q, %v)", got, found)
	}
}
