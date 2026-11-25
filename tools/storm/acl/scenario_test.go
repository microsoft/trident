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

	qemu "tridenttools/storm/utils/vm/qemu"
)

func TestGatherLogsRetriesFailedCollection(t *testing.T) {
	dir := t.TempDir()
	serial := filepath.Join(dir, "serial-console.log")
	if err := os.WriteFile(serial, []byte("serial evidence"), 0600); err != nil {
		t.Fatal(err)
	}
	s := ACLScenario{runner: &runner{
		args: scenarioArgs{OutputPath: dir, OperationTimeout: time.Second},
		qemu: qemu.QemuConfig{SerialLog: serial},
	}}
	if err := s.gatherLogs(); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("expected missing evidence directory, got %v", err)
	}
	if s.logsCollected {
		t.Fatal("failed collection was marked complete")
	}
	evidence := filepath.Join(dir, "evidence")
	if err := os.Mkdir(evidence, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(evidence, "command.log"), []byte("recovered diagnostics"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := s.gatherLogs(); err != nil {
		t.Fatal(err)
	}
	if !s.logsCollected || s.failure == nil {
		t.Fatal("retry must complete collection without erasing the original failure")
	}
	file, err := os.Open(filepath.Join(dir, "guest-evidence.tar.gz"))
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	reader, err := gzip.NewReader(file)
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	archive := tar.NewReader(reader)
	header, err := archive.Next()
	if err != nil {
		t.Fatal(err)
	}
	data, err := io.ReadAll(archive)
	if err != nil {
		t.Fatal(err)
	}
	if header.Name != "evidence/command.log" || string(data) != "recovered diagnostics" {
		t.Fatalf("retry did not publish recovered evidence: %q, %q", header.Name, data)
	}
	if err := os.Remove(serial); err != nil {
		t.Fatal(err)
	}
	if err := s.gatherLogs(); err != nil {
		t.Fatalf("successful collection should not run again: %v", err)
	}
}
