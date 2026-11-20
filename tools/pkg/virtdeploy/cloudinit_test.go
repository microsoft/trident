package virtdeploy

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestCloudInitISOContents(t *testing.T) {
	xorriso, err := exec.LookPath("xorriso")
	if err != nil {
		t.Skip("xorriso is needed to inspect the cloud-init ISO")
	}
	dir := t.TempDir()
	userdata := filepath.Join(dir, "source")
	want := "#cloud-config\nhostname: facilities-test\n"
	if err := os.WriteFile(userdata, []byte(want), 0600); err != nil {
		t.Fatal(err)
	}
	image := filepath.Join(dir, "seed.iso")
	if err := buildCloudInitIso(&CloudInitConfig{Userdata: userdata}, image); err != nil {
		t.Fatal(err)
	}
	extracted := filepath.Join(dir, "extracted")
	t.Cleanup(func() {
		if err := os.Chmod(extracted, 0700); err != nil && !os.IsNotExist(err) {
			t.Error(err)
		}
	})
	if out, err := exec.Command(xorriso, "-osirrox", "on", "-indev", image, "-extract", "/", extracted).CombinedOutput(); err != nil {
		t.Fatalf("inspect ISO: %v\n%s", err, out)
	}
	user, err := os.ReadFile(filepath.Join(extracted, "user-data"))
	if err != nil || string(user) != want {
		t.Fatalf("user-data: %q, %v", user, err)
	}
	metadata, err := os.ReadFile(filepath.Join(extracted, "meta-data"))
	if err != nil || len(metadata) != 0 {
		t.Fatalf("default meta-data: %q, %v", metadata, err)
	}
}

func TestCloudInitISORejectsMissingInput(t *testing.T) {
	if err := buildCloudInitIso(nil, filepath.Join(t.TempDir(), "seed.iso")); err == nil {
		t.Fatal("nil configuration was accepted")
	}
	if err := buildCloudInitIso(&CloudInitConfig{Userdata: filepath.Join(t.TempDir(), "missing")}, filepath.Join(t.TempDir(), "seed.iso")); err == nil {
		t.Fatal("missing user-data was accepted")
	}
}
