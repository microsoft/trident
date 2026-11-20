package acl

import (
	"archive/tar"
	"compress/gzip"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"time"

	"github.com/microsoft/storm"
	qemu "tridenttools/storm/utils/vm/qemu"
)

// collectAndSync attempts guest sync even when evidence collection fails.
func collectAndSync(collect, sync func() error) error {
	return errors.Join(collect(), sync())
}

// cleanupVMFiles removes only the disposable working disk after a successful run.
func cleanupVMFiles(artifactsDir string, successful bool) error {
	if !successful {
		return nil
	}
	err := os.Remove(filepath.Join(artifactsDir, "booted.qcow2"))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

// evidence captures current boot logs and archives previously saved command evidence.
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
	destination, err := os.OpenFile(filepath.Join(r.args.OutputPath, "guest-evidence.tar.gz"), os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0600)
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

// recordFailure preserves operation and cleanup errors for the final result.
func (s *ACLScenario) recordFailure(err error) {
	if err != nil && !errors.Is(s.failure, err) {
		s.failure = errors.Join(s.failure, err)
	}
}

// runTestCase records an operation failure before reporting it to Storm.
func (s *ACLScenario) runTestCase(tc storm.TestCase, operation func() error) error {
	if err := operation(); err != nil {
		s.recordFailure(err)
		tc.FailFromError(err)
	}
	return nil
}

// deployVm starts the guest used by the remaining ordered cases.
func (s *ACLScenario) deployVm(tc storm.TestCase) error {
	return s.runTestCase(tc, s.deploy)
}

// checkDeployment establishes the baseline boot and initial slot.
func (s *ACLScenario) checkDeployment(tc storm.TestCase) error {
	return s.runTestCase(tc, s.lifecycle.checkDeployment)
}

// runABUpdate uploads update inputs and commits the first transition to B.
func (s *ACLScenario) runABUpdate(tc storm.TestCase) error {
	return s.runTestCase(tc, func() error {
		if err := s.runner.prepareUpdate(); err != nil {
			return err
		}
		return s.lifecycle.update("update-1", "b", false)
	})
}

// runRollback commits a return to the retained A slot.
func (s *ACLScenario) runRollback(tc storm.TestCase) error {
	return s.runTestCase(tc, func() error { return s.lifecycle.update("rollback", "a", true) })
}

// runABUpdate2 verifies servicing can return to B after rollback.
func (s *ACLScenario) runABUpdate2(tc storm.TestCase) error {
	return s.runTestCase(tc, func() error { return s.lifecycle.update("update-2", "b", false) })
}

// runABUpdate3 verifies another update can reuse A.
func (s *ACLScenario) runABUpdate3(tc storm.TestCase) error {
	return s.runTestCase(tc, func() error { return s.lifecycle.update("update-3", "a", false) })
}

// checkPersistence marks the scenario complete only after a persisted-boot check.
func (s *ACLScenario) checkPersistence(tc storm.TestCase) error {
	return s.runTestCase(tc, func() error {
		if err := s.lifecycle.checkPersistence(); err != nil {
			return err
		}
		s.completed = true
		return nil
	})
}

// collectLogs captures diagnostics before the named cleanup case.
func (s *ACLScenario) collectLogs(tc storm.TestCase) error {
	return s.runTestCase(tc, s.gatherLogs)
}

// cleanupVm releases the guest and writes the final scenario result.
func (s *ACLScenario) cleanupVm(tc storm.TestCase) error {
	return s.runTestCase(tc, s.finish)
}

// OnFailure captures diagnostics before Storm pauses or cleans up a failed scenario.
func (s *ACLScenario) OnFailure(ctx storm.FailureContext) error {
	if s.failure == nil {
		for _, failed := range ctx.FailedCases() {
			s.recordFailure(fmt.Errorf("%s: %s", failed.Name(), failed.Reason()))
		}
	}
	return s.gatherLogs()
}

// gatherLogs collects diagnostics and syncs guest writes at most once.
func (s *ACLScenario) gatherLogs() error {
	if s.runner == nil || s.logsCollected {
		return nil
	}
	s.logsCollected = true
	r := s.runner
	err := collectAndSync(r.evidence, func() error {
		if r.ip == "" {
			return nil
		}
		previous := r.args.OperationTimeout
		r.args.OperationTimeout = min(previous, 30*time.Second)
		defer func() { r.args.OperationTimeout = previous }()
		_, err := r.command("sync")
		return err
	})
	s.recordFailure(err)
	return err
}

// finish releases resources and writes final status, retaining working disks on failure.
func (s *ACLScenario) finish() error {
	if s.runner == nil || s.finished {
		return nil
	}
	r := s.runner
	defer r.log.Close()
	var incompleteErr error
	if !s.completed && s.failure == nil {
		incompleteErr = errors.New("ACL servicing did not complete")
		s.recordFailure(incompleteErr)
	}
	evidenceErr := s.gatherLogs()
	r.closeClient()
	cleanupErr := r.qemu.CleanupQemuVM(r.vmName)
	if cleanupErr == nil {
		cleanupErr = cleanupVMFiles(r.args.ArtifactsDir, s.completed && s.failure == nil)
	}
	s.recordFailure(cleanupErr)
	summary := struct {
		Variant imageVariant `json:"variant"`
		VMName  string       `json:"vmName"`
		Passed  bool         `json:"passed"`
		Error   string       `json:"error,omitempty"`
	}{Variant: s.args.Variant, VMName: r.vmName, Passed: s.completed && s.failure == nil}
	if s.failure != nil {
		summary.Error = s.failure.Error()
	}
	raw, marshalErr := json.MarshalIndent(summary, "", "  ")
	summaryErr := errors.Join(marshalErr, os.WriteFile(filepath.Join(r.args.OutputPath, "result.json"), raw, 0600))
	s.recordFailure(summaryErr)
	s.finished = true
	return errors.Join(incompleteErr, evidenceErr, cleanupErr, summaryErr)
}

// deploy validates inputs, initializes evidence output and boots the disposable VM.
func (s *ACLScenario) deploy() error {
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
	if err := validateArtifacts(s.args.ArtifactsDir, s.args.Variant); err != nil {
		return err
	}
	if err := os.MkdirAll(s.args.OutputPath, 0755); err != nil {
		return err
	}
	log, err := os.OpenFile(filepath.Join(s.args.OutputPath, "controller.log"), os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	r := &runner{args: s.args, vmName: s.args.VMName, log: log}
	r.qemu = qemu.QemuConfig{NestedVirtualization: s.args.Variant == variantMSHV, SecureBoot: false, SerialLog: filepath.Join(s.args.OutputPath, "serial-console.log"), ImagePattern: `^base\.qcow2$`}
	if s.args.Variant == variantMSHV {
		r.qemu.MemoryMiB = 16 * 1024
		r.qemu.VCPUs = 4
	}
	s.runner = r
	s.lifecycle = lifecycle{g: r}
	ctx, cancel := context.WithTimeout(context.Background(), s.args.OperationTimeout)
	defer cancel()
	if err := r.qemu.DeployQemuVMWithoutWaiting(ctx, r.vmName, s.args.ArtifactsDir); err != nil {
		return err
	}
	if _, err := r.waitBoot(NO_PREVIOUS_BOOT_ID); err != nil {
		return err
	}
	return nil
}
