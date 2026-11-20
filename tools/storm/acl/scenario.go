package acl

import (
	"archive/tar"
	"compress/gzip"
	"context"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"time"

	"github.com/google/uuid"
	qemu "tridenttools/storm/utils/vm/qemu"
)

func collectAndSync(collect, sync func() error) error {
	return errors.Join(collect(), sync())
}

func cleanupVMFiles(runDir string, successful bool) error {
	names := []string{"base.qcow2"}
	if successful {
		names = append(names, "booted.qcow2")
	}
	var result error
	for _, name := range names {
		if err := os.Remove(filepath.Join(runDir, name)); err != nil && !errors.Is(err, os.ErrNotExist) {
			result = errors.Join(result, err)
		}
	}
	return result
}

func (r *runner) evidence() error {
	previous := r.args.OperationTimeout
	r.args.OperationTimeout = min(previous, 30*time.Second)
	defer func() { r.args.OperationTimeout = previous }()
	var result error
	if r.ip != "" {
		journal, err := r.command("journalctl -b --no-pager")
		result = errors.Join(result, err, r.save("failure-journal.log", []byte(journal)))
	}
	serial, err := os.ReadFile(r.qemu.SerialLog)
	if err == nil {
		err = os.WriteFile(filepath.Join(r.args.OutputPath, "serial.log"), serial, 0600)
	}
	result = errors.Join(result, err)
	destination, err := os.OpenFile(filepath.Join(r.args.OutputPath, "guest-evidence.tar.gz"), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return errors.Join(result, err)
	}
	compressed := gzip.NewWriter(destination)
	archive := tar.NewWriter(compressed)
	walkErr := filepath.WalkDir(filepath.Join(r.args.OutputPath, "evidence"), func(file string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			return nil
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		header, err := tar.FileInfoHeader(info, "")
		if err != nil {
			return err
		}
		relative, err := filepath.Rel(r.args.OutputPath, file)
		if err != nil {
			return err
		}
		header.Name = filepath.ToSlash(relative)
		if err := archive.WriteHeader(header); err != nil {
			return err
		}
		input, err := os.Open(file)
		if err != nil {
			return err
		}
		_, copyErr := io.Copy(archive, input)
		return errors.Join(copyErr, input.Close())
	})
	return errors.Join(result, walkErr, archive.Close(), compressed.Close(), destination.Close())
}

func (s *ACLScenario) run() (result error) {
	if s.args.BootTimeout <= 0 || s.args.OperationTimeout <= 0 {
		return errors.New("timeouts must be positive")
	}
	var err error
	s.args.ArtifactsDir, err = filepath.Abs(s.args.ArtifactsDir)
	if err != nil {
		return err
	}
	s.args.OutputPath, err = filepath.Abs(s.args.OutputPath)
	if err != nil {
		return err
	}
	if s.args.OutputPath == s.args.ArtifactsDir {
		return errors.New("output path must differ from immutable artifacts directory")
	}
	info, err := readArtifacts(s.args.ArtifactsDir, s.args.Variant)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(s.args.OutputPath, 0755); err != nil {
		return err
	}
	runDir := filepath.Join(s.args.OutputPath, "run-"+uuid.NewString())
	if err := os.Mkdir(runDir, 0755); err != nil {
		return err
	}
	s.args.OutputPath = runDir
	log, err := os.OpenFile(filepath.Join(runDir, "controller.log"), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	defer log.Close()
	r := &runner{args: s.args, vmName: s.args.VMName + "-" + uuid.NewString(), log: log, snapshots: make(map[string]snapshot)}
	r.qemu = qemu.QemuConfig{NestedVirtualization: s.args.Variant == "mshv", SecureBoot: false, SerialLog: filepath.Join(runDir, "serial-console.log"), ImagePattern: `^base\.qcow2$`}
	if s.args.Variant == "mshv" {
		r.qemu.MemoryMiB = 16 * 1024
		r.qemu.VCPUs = 4
	}
	defer func() {
		evidenceErr := collectAndSync(r.evidence, func() error {
			if r.ip == "" {
				return nil
			}
			// A failed guest must not turn diagnostic collection into another long boot wait.
			previous := r.args.OperationTimeout
			r.args.OperationTimeout = min(previous, 30*time.Second)
			defer func() { r.args.OperationTimeout = previous }()
			_, err := r.command("sync")
			return err
		})
		r.closeClient()
		cleanupErr := r.qemu.CleanupQemuVM(r.vmName)
		result = errors.Join(result, evidenceErr, cleanupErr)
		if cleanupErr == nil {
			result = errors.Join(result, cleanupVMFiles(runDir, result == nil))
		}
		summary := struct {
			artifactInfo
			VMName string `json:"vmName"`
			Passed bool   `json:"passed"`
			Error  string `json:"error,omitempty"`
		}{artifactInfo: info, VMName: r.vmName, Passed: result == nil}
		if result != nil {
			summary.Error = result.Error()
		}
		raw, marshalErr := json.MarshalIndent(summary, "", "  ")
		result = errors.Join(result, marshalErr, os.WriteFile(filepath.Join(runDir, "result.json"), raw, 0600))
	}()
	for _, name := range []string{"manifest.json", "metadata.json"} {
		data, err := os.ReadFile(filepath.Join(s.args.ArtifactsDir, name))
		if err != nil {
			return err
		}
		if err := os.WriteFile(filepath.Join(runDir, name), data, 0600); err != nil {
			return err
		}
	}
	// The QEMU helper copies beside its source; a private symlink keeps build inputs
	// immutable and isolates concurrent runs.
	if err := os.Symlink(filepath.Join(s.args.ArtifactsDir, "base.qcow2"), filepath.Join(runDir, "base.qcow2")); err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), s.args.OperationTimeout)
	defer cancel()
	if err := r.qemu.DeployQemuVMWithoutWaiting(ctx, r.vmName, runDir); err != nil {
		return err
	}
	if _, err := r.waitBoot(""); err != nil {
		return err
	}
	if _, err := r.command("mkdir -p " + guestDir); err != nil {
		return err
	}
	config := filepath.Join(runDir, "update.yaml")
	if err := os.WriteFile(config, []byte(updateConfig()), 0600); err != nil {
		return err
	}
	if err := r.upload(config, guestDir+"/update.yaml"); err != nil {
		return err
	}
	if err := r.upload(filepath.Join(s.args.ArtifactsDir, "update.cosi"), guestDir+"/update.cosi"); err != nil {
		return err
	}
	return service(r, info)
}
