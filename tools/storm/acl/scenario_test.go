package acl

import (
	"archive/tar"
	"compress/gzip"
	"errors"
	"io"
	"os"
	"path/filepath"
	"testing"
	"time"

	"tridenttools/storm/utils/vm/qemu"
)

func TestEvidenceRetainsEarlierCheckpointsWithoutGuestSSH(t *testing.T) {
	dir := t.TempDir()
	serial := filepath.Join(dir, "serial-console.log")
	if err := os.WriteFile(serial, []byte("reboot requested\n"), 0600); err != nil {
		t.Fatal(err)
	}
	r := &runner{args: scenarioArgs{OutputPath: dir, OperationTimeout: time.Minute}, qemu: qemu.QemuConfig{SerialLog: serial}}
	const captured = "servicingState: ab-update-staged\n"
	if err := r.save("update-1-staged/status", []byte(captured)); err != nil {
		t.Fatal(err)
	}
	if err := r.evidence(); err != nil {
		t.Fatal(err)
	}
	file, err := os.Open(filepath.Join(dir, "guest-evidence.tar.gz"))
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	compressed, err := gzip.NewReader(file)
	if err != nil {
		t.Fatal(err)
	}
	defer compressed.Close()
	archive := tar.NewReader(compressed)
	found := false
	for {
		header, err := archive.Next()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		if header.Name == "evidence/update-1-staged/status" {
			data, err := io.ReadAll(archive)
			if err != nil || string(data) != captured {
				t.Fatalf("checkpoint lost: %q, %v", data, err)
			}
			found = true
		}
	}
	if !found {
		t.Fatal("checkpoint missing from archive")
	}
	data, err := os.ReadFile(filepath.Join(dir, "serial.log"))
	if err != nil || string(data) != "reboot requested\n" {
		t.Fatalf("serial lost: %q, %v", data, err)
	}
}
