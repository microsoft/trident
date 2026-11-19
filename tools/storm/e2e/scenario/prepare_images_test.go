package scenario

import (
	"os"
	"path/filepath"
	"testing"
)

// newImagePrepScenario builds a scenario whose TestImageDir is a temp dir
// pre-seeded with distinct v1 (regular.cosi) and v2 (regular_v2.cosi) images.
func newImagePrepScenario(t *testing.T, dir string) *TridentE2EScenario {
	t.Helper()
	for name, content := range map[string]string{
		"regular.cosi":    "v1-content",
		"regular_v2.cosi": "v2-content",
	} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0644); err != nil {
			t.Fatalf("seed image %s: %v", name, err)
		}
	}
	s := &TridentE2EScenario{}
	s.args.TestImageDir = dir
	return s
}

func TestEnsureVersionedImage_HardlinkScheme(t *testing.T) {
	dir := t.TempDir()
	s := newImagePrepScenario(t, dir)

	if err := s.ensureVersionedImage("regular", "cosi", 3); err != nil {
		t.Fatalf("ensure v3: %v", err)
	}
	if err := s.ensureVersionedImage("regular", "cosi", 4); err != nil {
		t.Fatalf("ensure v4: %v", err)
	}

	// v3 (odd) must alias v1 (regular.cosi); v4 (even) must alias v2.
	assertSameContent(t, dir, "regular_v3.cosi", "v1-content")
	assertSameContent(t, dir, "regular_v4.cosi", "v2-content")

	// They must be hard-links (same inode) to their source, not copies.
	assertSameInode(t, filepath.Join(dir, "regular_v3.cosi"), filepath.Join(dir, "regular.cosi"))
	assertSameInode(t, filepath.Join(dir, "regular_v4.cosi"), filepath.Join(dir, "regular_v2.cosi"))
}

func TestEnsureVersionedImage_ExistingLeftAsIs(t *testing.T) {
	dir := t.TempDir()
	s := newImagePrepScenario(t, dir)

	// Pre-existing v3 with distinct content must not be overwritten.
	existing := filepath.Join(dir, "regular_v3.cosi")
	if err := os.WriteFile(existing, []byte("real-v3"), 0644); err != nil {
		t.Fatal(err)
	}
	if err := s.ensureVersionedImage("regular", "cosi", 3); err != nil {
		t.Fatalf("ensure v3: %v", err)
	}
	assertSameContent(t, dir, "regular_v3.cosi", "real-v3")
}

func TestEnsureVersionedImage_MissingSourceFails(t *testing.T) {
	dir := t.TempDir()
	s := &TridentE2EScenario{}
	s.args.TestImageDir = dir
	// No base images seeded.
	if err := s.ensureVersionedImage("regular", "cosi", 3); err == nil {
		t.Error("expected an error when the source base image is missing")
	}
}

func assertSameContent(t *testing.T, dir, name, want string) {
	t.Helper()
	got, err := os.ReadFile(filepath.Join(dir, name))
	if err != nil {
		t.Fatalf("read %s: %v", name, err)
	}
	if string(got) != want {
		t.Errorf("%s content = %q, want %q", name, got, want)
	}
}

func assertSameInode(t *testing.T, a, b string) {
	t.Helper()
	ai, err := os.Stat(a)
	if err != nil {
		t.Fatal(err)
	}
	bi, err := os.Stat(b)
	if err != nil {
		t.Fatal(err)
	}
	if !os.SameFile(ai, bi) {
		t.Errorf("%s and %s are not the same file (expected a hard link)", a, b)
	}
}
