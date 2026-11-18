// Package acl exercises manual adoption and A/B servicing of synthetic ACL guests.
package acl

import (
	"archive/tar"
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/sha512"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/microsoft/storm"
	metadata "tridenttools/cmd/mkcosi/metadata"
	qemu "tridenttools/storm/utils/vm/qemu"
)

//go:embed guest-test.sh
var guestAssertions string

const guestDir = "/var/lib/trident/synthetic-acl"

var errDisconnected = errors.New("SSH disconnected")

type SyntheticACLScenario struct{ args scenarioArgs }
type scenarioArgs struct {
	Variant          string        `help:"Synthetic ACL image variant" enum:"standard,mshv" default:"standard"`
	ArtifactsDir     string        `help:"Directory containing synthetic ACL base.qcow2, update.cosi, metadata.json, manifest.json and id_rsa" required:""`
	OutputPath       string        `help:"Directory for evidence and private VM working disk" required:""`
	VMName           string        `help:"Prefix for a uniquely named disposable QEMU VM" default:"synthetic-acl"`
	BootTimeout      time.Duration `help:"Maximum wait for each SSH-ready boot" default:"10m"`
	OperationTimeout time.Duration `help:"Maximum duration of each deployment or guest operation" default:"30m"`
}

func (s *SyntheticACLScenario) Name() string                            { return "synthetic-acl" }
func (s *SyntheticACLScenario) Args() any                               { return &s.args }
func (s *SyntheticACLScenario) Tags() []string                          { return []string{"qemu", "acl"} }
func (s *SyntheticACLScenario) StagePaths() []string                    { return nil }
func (s *SyntheticACLScenario) RequiredFiles() []string                 { return nil }
func (s *SyntheticACLScenario) Setup(storm.SetupCleanupContext) error   { return nil }
func (s *SyntheticACLScenario) Cleanup(storm.SetupCleanupContext) error { return nil }
func (s *SyntheticACLScenario) RegisterTestCases(r storm.TestRegistrar) error {
	r.RegisterTestCase("manual-ab-update", func(tc storm.TestCase) error {
		if err := s.run(); err != nil {
			tc.FailFromError(err)
		}
		return nil
	})
	return nil
}

type artifactInfo struct {
	Variant        string `json:"variant"`
	MetadataSHA384 string `json:"metadataSha384"`
	CosiSHA256     string `json:"cosiSha256"`
	TargetUUID     string `json:"targetUsrUuid"`
	TargetHash     string `json:"targetRootHash"`
	TridentSHA256  string `json:"expectedTridentSha256"`
}

func readArtifacts(dir, variant string) (artifactInfo, error) {
	info := artifactInfo{Variant: variant}
	if variant != "standard" && variant != "mshv" {
		return info, fmt.Errorf("unsupported synthetic ACL variant %q; expected standard or mshv", variant)
	}
	for _, name := range []string{"base.qcow2", "update.cosi", "metadata.json", "manifest.json", "id_rsa"} {
		st, err := os.Stat(filepath.Join(dir, name))
		if err != nil {
			return info, err
		}
		if !st.Mode().IsRegular() || st.Size() == 0 {
			return info, fmt.Errorf("%s must be a nonempty regular file", name)
		}
	}
	raw, err := os.ReadFile(filepath.Join(dir, "metadata.json"))
	if err != nil {
		return info, err
	}
	var m metadata.MetadataJson
	if err = json.Unmarshal(raw, &m); err != nil {
		return info, fmt.Errorf("COSI metadata: %w", err)
	}
	validBootloader := m.Bootloader.Type == metadata.BootloaderTypeSystemDBoot && (m.Version == "1.2" || m.Version == "1.3")
	if variant == "mshv" {
		validBootloader = m.Bootloader.Type == "efitreeloader-mshv-systemd-boot" && m.Version == "1.3"
	}
	if !validBootloader || m.OsArch != metadata.OsArchitectureX8664 {
		return info, fmt.Errorf("incompatible %s COSI metadata: version=%q osArch=%q bootloader=%q", variant, m.Version, m.OsArch, m.Bootloader.Type)
	}
	count := 0
	for _, image := range m.Images {
		if image.MountPoint != "/usr" {
			continue
		}
		count++
		id, err := uuid.Parse(image.FsUuid)
		if err != nil || id == uuid.Nil || image.Verity == nil || !isDigest(image.Verity.Roothash, 32) {
			return info, fmt.Errorf("expected verity-backed /usr with nonzero filesystem UUID and SHA256 root hash")
		}
		info.TargetUUID = id.String()
		info.TargetHash = strings.ToLower(image.Verity.Roothash)
	}
	if count != 1 {
		return info, fmt.Errorf("expected exactly one /usr image, got %d", count)
	}
	hash := sha512.Sum384(raw)
	info.MetadataSHA384 = hex.EncodeToString(hash[:])
	var manifest struct {
		Synthetic      bool   `json:"synthetic"`
		Variant        string `json:"variant"`
		MetadataSHA384 string `json:"metadataSha384"`
		TridentSHA256  string `json:"expectedTridentSha256"`
	}
	manifestBytes, err := os.ReadFile(filepath.Join(dir, "manifest.json"))
	if err != nil {
		return info, err
	}
	if err = json.Unmarshal(manifestBytes, &manifest); err != nil {
		return info, err
	}
	if !manifest.Synthetic || manifest.Variant != variant || !isDigest(manifest.TridentSHA256, 32) || !strings.EqualFold(manifest.MetadataSHA384, info.MetadataSHA384) {
		return info, fmt.Errorf("manifest must identify synthetic %s ACL, expected Trident SHA256 and exact metadata SHA384", variant)
	}
	info.TridentSHA256 = strings.ToLower(manifest.TridentSHA256)
	f, err := os.Open(filepath.Join(dir, "update.cosi"))
	if err != nil {
		return info, err
	}
	defer f.Close()
	tr := tar.NewReader(f)
	for {
		header, err := tr.Next()
		if err != nil {
			return info, fmt.Errorf("COSI metadata member: %w", err)
		}
		if header.Name != "metadata.json" {
			continue
		}
		if header.Typeflag != tar.TypeReg || header.Size != int64(len(raw)) {
			return info, fmt.Errorf("COSI metadata member differs from metadata.json")
		}
		embedded, err := io.ReadAll(io.LimitReader(tr, int64(len(raw))+1))
		if err != nil {
			return info, err
		}
		if !bytes.Equal(embedded, raw) {
			return info, fmt.Errorf("COSI metadata member differs from metadata.json bytes")
		}
		break
	}
	if _, err = f.Seek(0, io.SeekStart); err != nil {
		return info, err
	}
	h := sha256.New()
	if _, err = io.Copy(h, f); err != nil {
		return info, err
	}
	info.CosiSHA256 = hex.EncodeToString(h.Sum(nil))
	return info, nil
}
func isDigest(value string, length int) bool {
	b, err := hex.DecodeString(value)
	return err == nil && len(b) == length
}
func quote(s string) string { return "'" + strings.ReplaceAll(s, "'", "'\"'\"'") + "'" }

func updateConfig(info artifactInfo) string {
	return fmt.Sprintf("image:\n  url: file://%s/update.cosi\n  sha384: %s\ninternalParams:\n  forceAbUpdate: true\nos:\n  uefiFallback: conservative\n", guestDir, info.MetadataSHA384)
}

type guest interface {
	command(string) (string, error)
	waitBoot(string) (string, error)
}

// Manual invocations deliberately separate stage, finalize and explicit commit.
// A rebooting SSH command may disconnect; only a new verified boot proves success.
func service(g guest, info artifactInfo) error {
	assert := func(phase string) error {
		_, err := g.command(fmt.Sprintf("bash %s/guest-test.sh %s %s %s %s %s", guestDir, phase, info.Variant, info.TargetUUID, info.TargetHash, info.TridentSHA256))
		return err
	}
	baseline, err := g.waitBoot("")
	if err != nil {
		return fmt.Errorf("baseline SSH: %w", err)
	}
	if err = assert("baseline"); err != nil {
		return fmt.Errorf("baseline: %w", err)
	}
	if _, err = g.command(fmt.Sprintf("test \"$(sha256sum %s/update.cosi | cut -d ' ' -f 1)\" = %s", guestDir, quote(info.CosiSHA256))); err != nil {
		return err
	}
	if _, err = g.command(fmt.Sprintf("trident update %s/update.yaml --allowed-operations stage > %s/stage.log 2>&1", guestDir, guestDir)); err != nil {
		return fmt.Errorf("stage: %w", err)
	}
	if err = assert("staged"); err != nil {
		return fmt.Errorf("staged: %w", err)
	}
	_, finalizeErr := g.command(fmt.Sprintf("trident update %s/update.yaml --allowed-operations finalize > %s/finalize.log 2>&1", guestDir, guestDir))
	if finalizeErr != nil && !errors.Is(finalizeErr, errDisconnected) {
		return fmt.Errorf("finalize: %w", finalizeErr)
	}
	updated, err := g.waitBoot(baseline)
	if err != nil {
		return errors.Join(fmt.Errorf("finalize boot: %w", err), finalizeErr)
	}
	if err = assert("updated"); err != nil {
		return fmt.Errorf("updated: %w", err)
	}
	if _, err = g.command(fmt.Sprintf("trident commit > %s/commit.log 2>&1", guestDir)); err != nil {
		return fmt.Errorf("commit: %w", err)
	}
	if err = assert("committed"); err != nil {
		return fmt.Errorf("committed: %w", err)
	}
	_, rebootErr := g.command("systemctl reboot")
	if rebootErr != nil && !errors.Is(rebootErr, errDisconnected) {
		return fmt.Errorf("persisted reboot: %w", rebootErr)
	}
	if _, err = g.waitBoot(updated); err != nil {
		return errors.Join(fmt.Errorf("persisted boot: %w", err), rebootErr)
	}
	if err = assert("persisted"); err != nil {
		return fmt.Errorf("persisted: %w", err)
	}
	return nil
}

type runner struct {
	args   scenarioArgs
	vmName string
	ip     string
	qemu   qemu.QemuConfig
	log    *os.File
}

func (r *runner) local(timeout time.Duration, name string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, name, args...)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	output, err := cmd.Output()
	if _, logErr := fmt.Fprintf(r.log, "$ %s %s\nstdout:\n%s\nstderr:\n%s\nerror: %v\n", name, strings.Join(args, " "), output, stderr.Bytes(), err); logErr != nil {
		return string(output), errors.Join(err, logErr)
	}
	if ctx.Err() != nil {
		return string(output), ctx.Err()
	}
	return strings.TrimSpace(string(output)), err
}
func (r *runner) sshOptions() []string {
	return []string{"-i", filepath.Join(r.args.ArtifactsDir, "id_rsa"), "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-o", "UserKnownHostsFile=" + filepath.Join(r.args.OutputPath, "known_hosts"), "-o", "ConnectTimeout=5", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3"}
}
func (r *runner) command(command string) (string, error) {
	output, err := r.local(r.args.OperationTimeout, "ssh", append(r.sshOptions(), "root@"+r.ip, command)...)
	var exit *exec.ExitError
	if errors.As(err, &exit) && exit.ExitCode() == 255 {
		return output, errors.Join(errDisconnected, err)
	}
	return output, err
}
func (r *runner) upload(source, dest string) error {
	_, err := r.local(r.args.OperationTimeout, "scp", append(r.sshOptions(), source, "root@"+r.ip+":"+dest)...)
	return err
}
func (r *runner) waitBoot(previous string) (string, error) {
	deadline := time.Now().Add(r.args.BootTimeout)
	var lastErr error
	for time.Now().Before(deadline) {
		if r.ip == "" {
			ips, err := r.qemu.GetVmIPAddresses(r.vmName)
			lastErr = err
			if len(ips) == 1 {
				r.ip = ips[0]
			}
		}
		if r.ip != "" {
			output, err := r.local(10*time.Second, "ssh", append(r.sshOptions(), "root@"+r.ip, "cat /proc/sys/kernel/random/boot_id")...)
			lastErr = err
			if err == nil {
				if id, parseErr := uuid.Parse(output); parseErr == nil && id != uuid.Nil && output != previous {
					return output, nil
				}
			}
		}
		time.Sleep(2 * time.Second)
	}
	return "", fmt.Errorf("timed out waiting for boot different from %q: %v", previous, lastErr)
}
func (r *runner) evidence() error {
	var errs []error
	if r.ip != "" {
		_, err := r.command("mkdir -p " + guestDir + " /var/lib/trident/test-evidence; journalctl -b --no-pager > " + guestDir + "/failure-journal.log; tar -czf /var/lib/trident/synthetic-acl-evidence.tar.gz /var/lib/trident/test-evidence " + guestDir + "/*.log")
		errs = append(errs, err)
		_, err = r.local(r.args.OperationTimeout, "scp", append(r.sshOptions(), "root@"+r.ip+":/var/lib/trident/synthetic-acl-evidence.tar.gz", filepath.Join(r.args.OutputPath, "guest-evidence.tar.gz"))...)
		errs = append(errs, err)
	} else {
		errs = append(errs, errors.New("guest unreachable: no guest evidence collected"))
	}
	if data, err := os.ReadFile(r.qemu.SerialLog); err != nil {
		errs = append(errs, err)
	} else {
		errs = append(errs, os.WriteFile(filepath.Join(r.args.OutputPath, "serial.log"), data, 0600))
	}
	return errors.Join(errs...)
}

// Flush writes even if evidence collection failed, before destroying the domain.
func collectAndSync(collect, sync func() error) error {
	evidenceErr := collect()
	syncErr := sync()
	return errors.Join(evidenceErr, syncErr)
}

// Preserve failed guests' disks for diagnosis after their domain is removed.
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

func (s *SyntheticACLScenario) run() (result error) {
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
	// An exclusive run directory prevents collisions or accidental deletion of another VM.
	if err = os.MkdirAll(s.args.OutputPath, 0755); err != nil {
		return err
	}
	runDir := filepath.Join(s.args.OutputPath, "run-"+uuid.NewString())
	if err = os.Mkdir(runDir, 0755); err != nil {
		return err
	}
	s.args.OutputPath = runDir
	log, err := os.OpenFile(filepath.Join(runDir, "controller.log"), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	defer log.Close()
	r := &runner{args: s.args, vmName: s.args.VMName + "-" + uuid.NewString(), log: log}
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
			_, err := r.local(min(r.args.OperationTimeout, 30*time.Second), "ssh", append(r.sshOptions(), "root@"+r.ip, "sync")...)
			return err
		})
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
		if err = os.WriteFile(filepath.Join(runDir, name), data, 0600); err != nil {
			return err
		}
	}
	// The shared QEMU helper copies its source disk next to the source image.
	// A private symlink preserves immutable build inputs and parallel-run isolation.
	if err = os.Symlink(filepath.Join(s.args.ArtifactsDir, "base.qcow2"), filepath.Join(runDir, "base.qcow2")); err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), s.args.OperationTimeout)
	defer cancel()
	if err = r.qemu.DeployQemuVMWithoutWaiting(ctx, r.vmName, runDir); err != nil {
		return err
	}
	if _, err = r.waitBoot(""); err != nil {
		return err
	}
	if _, err = r.command("mkdir -p " + guestDir); err != nil {
		return err
	}
	for name, data := range map[string]string{"guest-test.sh": guestAssertions, "update.yaml": updateConfig(info)} {
		local := filepath.Join(runDir, name)
		if err = os.WriteFile(local, []byte(data), 0600); err != nil {
			return err
		}
		if err = r.upload(local, guestDir+"/"+name); err != nil {
			return err
		}
	}
	if err = r.upload(filepath.Join(s.args.ArtifactsDir, "update.cosi"), guestDir+"/update.cosi"); err != nil {
		return err
	}
	return service(r, info)
}
