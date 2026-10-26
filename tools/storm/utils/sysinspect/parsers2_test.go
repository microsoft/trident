package sysinspect

import "testing"

const mdSample = `total 0
lrwxrwxrwx 1 root root 8 Apr  1 22:42 home -> ../md124
lrwxrwxrwx 1 root root 8 Apr  1 22:42 root-a -> ../md127
lrwxrwxrwx 1 root root 8 Apr  1 22:42 root-b -> ../md125
lrwxrwxrwx 1 root root 8 Apr  1 22:42 trident -> ../md126`

func TestParseRaidName(t *testing.T) {
	name, ok := parseRaidName(mdSample, "/dev/md127")
	if !ok {
		t.Fatal("expected to resolve /dev/md127")
	}
	if name != "/dev/md/root-a" {
		t.Errorf("got %q, want /dev/md/root-a", name)
	}

	// Bare name form.
	name, ok = parseRaidName(mdSample, "md125")
	if !ok || name != "/dev/md/root-b" {
		t.Errorf("got %q (ok=%v), want /dev/md/root-b", name, ok)
	}

	// Unknown device.
	if _, ok := parseRaidName(mdSample, "/dev/md999"); ok {
		t.Error("md999 should not resolve")
	}
}

const passwdSample = `root:x:0:0:root:/root:/bin/bash
bin:x:1:1:bin:/dev/null:/bin/false
testing-user:x:1001:1001::/home/testing-user:/bin/bash`

const groupSample = `root:x:0:
wheel:x:10:testing-user,admin
bin:x:1:daemon
empty:x:99:`

func TestParsePasswd(t *testing.T) {
	users := ParsePasswd(passwdSample)
	if len(users) != 3 {
		t.Fatalf("got %d users, want 3", len(users))
	}
	if _, ok := users["testing-user"]; !ok {
		t.Error("testing-user missing")
	}
}

func TestParseGroup(t *testing.T) {
	groups := ParseGroup(groupSample)
	wheel, ok := groups["wheel"]
	if !ok {
		t.Fatal("wheel group missing")
	}
	if _, ok := wheel["testing-user"]; !ok {
		t.Error("testing-user not in wheel")
	}
	if _, ok := wheel["admin"]; !ok {
		t.Error("admin not in wheel")
	}
	if len(groups["empty"]) != 0 {
		t.Errorf("empty group should have no members, got %v", groups["empty"])
	}
}

const efiSample = `BootCurrent: 0001
Timeout: 0 seconds
BootOrder: 0001,0000
Boot0000* UiApp	FvVol(...)
Boot0001* azl	HD(1,GPT,abc)/File(...)`

func TestParseEfiBootMgr(t *testing.T) {
	info := ParseEfiBootMgr(efiSample)
	if info.BootCurrent != "0001" {
		t.Errorf("BootCurrent = %q, want 0001", info.BootCurrent)
	}
	name, ok := info.CurrentName()
	if !ok {
		t.Fatal("CurrentName not found")
	}
	if name != "azl" {
		t.Errorf("CurrentName = %q, want azl", name)
	}
	if info.EntryNames["0000"] != "UiApp" {
		t.Errorf("entry 0000 = %q, want UiApp", info.EntryNames["0000"])
	}
}
