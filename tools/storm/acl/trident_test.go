package acl

import (
	"archive/tar"
	"crypto/sha512"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

const testMetadata = `{
  "version":"1.2", "osArch":"x86_64", "bootloader":{"type":"systemd-boot"},
  "images":[{"mountPoint":"/usr","fsUuid":"a2d99335-7473-4f69-9858-acb3319f1d89",
  "verity":{"roothash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}]
}`

func fixture(t *testing.T, raw []byte) string {
	t.Helper()
	// Keep disposable fixture files inside the checkout, including on shared hosts.
	dir := ".acl-test-" + uuid.NewString()
	if err := os.Mkdir(dir, 0700); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(dir); err != nil {
			t.Error(err)
		}
	})
	for name, data := range map[string][]byte{"base.qcow2": []byte("fixture"), "id_rsa": []byte("fixture"), "metadata.json": raw} {
		if err := os.WriteFile(filepath.Join(dir, name), data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	sum := sha512.Sum384(raw)
	manifest := map[string]any{"synthetic": true, "variant": "standard", "metadataSha384": hex.EncodeToString(sum[:]), "expectedTridentSha256": strings.Repeat("b", 64)}
	data, err := json.Marshal(manifest)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(dir, "manifest.json"), data, 0600); err != nil {
		t.Fatal(err)
	}
	archive(t, dir, raw)
	return dir
}
func archive(t *testing.T, dir string, raw []byte) {
	t.Helper()
	f, err := os.Create(filepath.Join(dir, "update.cosi"))
	if err != nil {
		t.Fatal(err)
	}
	tw := tar.NewWriter(f)
	if err = tw.WriteHeader(&tar.Header{Name: "metadata.json", Mode: 0600, Size: int64(len(raw)), Typeflag: tar.TypeReg}); err != nil {
		t.Fatal(err)
	}
	if _, err = tw.Write(raw); err != nil {
		t.Fatal(err)
	}
	if err = tw.Close(); err != nil {
		t.Fatal(err)
	}
	if err = f.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestMetadataHashUsesExactRawBytes(t *testing.T) {
	raw := []byte(testMetadata + "\n")
	dir := fixture(t, raw)
	info, err := readArtifacts(dir, "standard")
	if err != nil {
		t.Fatal(err)
	}
	sum := sha512.Sum384(raw)
	if info.MetadataSHA384 != hex.EncodeToString(sum[:]) {
		t.Fatal("checksum did not preserve metadata whitespace")
	}
	if !strings.Contains(updateConfig(info), "sha384: "+info.MetadataSHA384) || !strings.Contains(updateConfig(info), "forceAbUpdate: true") || !strings.Contains(updateConfig(info), "noTransition: true") {
		t.Fatal("adoption config missing checksum, forced A/B update or manual transition")
	}
	// Same JSON document but different raw bytes is a different COSI checksum.
	archive(t, dir, []byte(testMetadata))
	if _, err = readArtifacts(dir, "standard"); err == nil {
		t.Fatal("accepted sidecar differing from archived metadata bytes")
	}
}
func TestRejectInvalidStandardArtifacts(t *testing.T) {
	tests := []struct{ name, from, to string }{
		{"mshv bootloader", "systemd-boot", "efitreeloader-mshv-systemd-boot"},
		{"unsupported version", "1.2", "1.9"},
		{"wrong architecture", "x86_64", "arm64"},
		{"missing usr", "/usr", "/"},
		{"missing verity", "\"verity\":{\"roothash\":\"" + strings.Repeat("a", 64) + "\"}", "\"verity\":null"},
		{"invalid UUID", "a2d99335-7473-4f69-9858-acb3319f1d89", "not-a-uuid"},
		{"invalid root hash", strings.Repeat("a", 64), "not-a-hash"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			dir := fixture(t, []byte(strings.Replace(testMetadata, tt.from, tt.to, 1)))
			if _, err := readArtifacts(dir, "standard"); err == nil {
				t.Fatal("accepted invalid metadata")
			}
		})
	}
	t.Run("wrong manifest variant", func(t *testing.T) {
		dir := fixture(t, []byte(testMetadata))
		p := filepath.Join(dir, "manifest.json")
		raw, err := os.ReadFile(p)
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(p, []byte(strings.Replace(string(raw), "standard", "mshv", 1)), 0600); err != nil {
			t.Fatal(err)
		}
		if _, err = readArtifacts(dir, "standard"); err == nil {
			t.Fatal("accepted MSHV provenance for Linux standard guest")
		}
	})
}

type fakeGuest struct {
	failErr          error
	pendingVolume    string
	calls            []string
	failCommand      string
	failOccurrence   int
	commandMatches   int
	failBoot         int
	boots            int
	disconnect       bool
	volume           string
	status           string
	statusCheckpoint string
	statusOutput     string
}

func (f *fakeGuest) command(command string) (string, error) {
	f.calls = append(f.calls, command)
	if f.failCommand != "" && strings.Contains(command, f.failCommand) {
		f.commandMatches++
		if f.failOccurrence == 0 || f.commandMatches == f.failOccurrence {
			if f.failErr != nil {
				return "", f.failErr
			}
			return "", errors.New("guest assertion or operation failed")
		}
	}
	if f.volume == "" {
		f.volume, f.status = "volume-a", "provisioned"
	}
	if strings.Contains(command, "--allowed-operations stage") {
		f.status = "ab-update-staged"
	}
	rebooting := strings.Contains(command, "--allowed-operations finalize") || strings.HasPrefix(command, "trident rollback")
	if rebooting {
		if f.volume == "volume-a" {
			f.pendingVolume = "volume-b"
		} else {
			f.pendingVolume = "volume-a"
		}
		f.status = "ab-update-finalized"
	}
	if strings.HasPrefix(command, "trident commit") {
		f.status = "provisioned"
	}
	if strings.HasPrefix(command, "cat /var/lib/trident/test-evidence/") {
		if f.statusCheckpoint != "" && strings.Contains(command, "/"+f.statusCheckpoint+"/status") {
			return f.statusOutput, nil
		}
		return fmt.Sprintf("servicingState: %s\nabActiveVolume: %s\n", f.status, f.volume), nil
	}
	if f.disconnect && (strings.HasPrefix(command, "trident rollback") || command == "systemctl reboot") {
		return "", errDisconnected
	}
	return "", nil
}
func (f *fakeGuest) waitBoot(previous string) (string, error) {
	f.boots++
	f.calls = append(f.calls, "wait:"+previous)
	if f.boots == f.failBoot {
		return "", errors.New("no new boot")
	}
	if f.pendingVolume != "" {
		f.volume = f.pendingVolume
		f.pendingVolume = ""
	}
	return fmt.Sprintf("boot-%d", f.boots), nil
}
func TestManualServicingUpdatesRollsBackAndUpdatesBothSlots(t *testing.T) {
	g := &fakeGuest{disconnect: true}
	if err := service(g, artifactInfo{Variant: "standard"}); err != nil {
		t.Fatal(err)
	}
	calls := strings.Join(g.calls, "\n")
	steps := []string{
		"guest-test.sh baseline standard",
		"--allowed-operations stage",
		"guest-test.sh update-1-staged standard",
		"--allowed-operations finalize",
		"systemctl reboot",
		"wait:boot-1",
		"trident commit",
		"guest-test.sh update-1 standard",
		"cat /var/lib/trident/test-evidence/update-1/status",
		"trident rollback --ab",
		"wait:boot-2",
		"trident commit",
		"guest-test.sh rollback standard",
		"cat /var/lib/trident/test-evidence/rollback/status",
		"--allowed-operations stage",
		"guest-test.sh update-2-staged standard",
		"--allowed-operations finalize",
		"systemctl reboot",
		"wait:boot-3",
		"trident commit",
		"guest-test.sh update-2 standard",
		"cat /var/lib/trident/test-evidence/update-2/status",
		"--allowed-operations stage",
		"guest-test.sh update-3-staged standard",
		"--allowed-operations finalize",
		"systemctl reboot",
		"wait:boot-4",
		"trident commit",
		"guest-test.sh update-3 standard",
		"cat /var/lib/trident/test-evidence/update-3/status",
		"systemctl reboot",
		"wait:boot-5",
		"guest-test.sh persisted standard",
	}
	remaining := calls
	for _, step := range steps {
		i := strings.Index(remaining, step)
		if i < 0 {
			t.Fatalf("missing/out-of-order step %q: %s", step, calls)
		}
		remaining = remaining[i+len(step):]
	}
	if strings.Count(calls, "trident commit") != 4 || g.boots != 6 {
		t.Fatalf("missing explicit commits or verified boots: %s", calls)
	}
	if strings.Contains(calls, "test-evidence/baseline/status") {
		t.Fatal("queried initial status before native ACL adoption")
	}
}
func TestManualServicingRejectsFinalizeDisconnectBeforeExplicitReboot(t *testing.T) {
	g := &fakeGuest{failCommand: "--allowed-operations finalize", failErr: errDisconnected}
	if err := service(g, artifactInfo{Variant: "standard"}); !errors.Is(err, errDisconnected) {
		t.Fatalf("unexpected finalize disconnect accepted: %v", err)
	}
	if strings.Contains(strings.Join(g.calls, "\n"), "systemctl reboot") || g.boots != 1 {
		t.Fatalf("rebooted after incomplete finalize: %v", g.calls)
	}
}
func TestManualServicingStopsOnFailure(t *testing.T) {
	tests := []struct {
		name, command string
		boot          int
		occurrence    int
		absent        string
	}{
		{name: "stage", command: "--allowed-operations stage", absent: "--allowed-operations finalize"},
		{name: "staged evidence", command: "guest-test.sh update-1-staged", absent: "--allowed-operations finalize"},
		{name: "finalize operation failure", command: "--allowed-operations finalize", absent: "trident commit"},
		{name: "manual reboot operation failure", command: "systemctl reboot", absent: "wait:boot-1"},
		{name: "finalize does not reboot", boot: 2, absent: "trident commit"},
		{name: "wrong updated identity", command: "guest-test.sh update-1 standard", absent: "trident rollback"},
		{name: "explicit commit", command: "trident commit", absent: "trident rollback"},
		{name: "rollback command", command: "trident rollback", absent: "guest-test.sh rollback"},
		{name: "rollback reboot missing", boot: 3, absent: "guest-test.sh rollback"},
		{name: "rollback commit", command: "trident commit", occurrence: 2, absent: "guest-test.sh rollback"},
		{name: "post rollback update", command: "/update-2-stage.log", absent: "guest-test.sh update-2-staged"},
		{name: "final update", command: "/update-3-finalize.log", absent: "guest-test.sh update-3 standard"},
		{name: "persisted reboot missing", boot: 6, absent: "guest-test.sh persisted"},
		{name: "persisted evidence", command: "guest-test.sh persisted"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			g := &fakeGuest{failCommand: tt.command, failBoot: tt.boot, failOccurrence: tt.occurrence}
			if err := service(g, artifactInfo{Variant: "standard"}); err == nil {
				t.Fatal("scenario passed despite required operation or evidence failure")
			}
			if tt.absent != "" && strings.Contains(strings.Join(g.calls, "\n"), tt.absent) {
				t.Fatalf("continued after failure: %v", g.calls)
			}
		})
	}
}

func TestCleanupPreservesBootDiskOnlyOnFailure(t *testing.T) {
	for _, successful := range []bool{false, true} {
		name := "failure"
		if successful {
			name = "success"
		}
		t.Run(name, func(t *testing.T) {
			dir := fixture(t, []byte(testMetadata))
			disk := filepath.Join(dir, "booted.qcow2")
			contents := []byte("failed guest disk evidence")
			if err := os.WriteFile(disk, contents, 0600); err != nil {
				t.Fatal(err)
			}
			if err := cleanupVMFiles(dir, successful); err != nil {
				t.Fatal(err)
			}
			if _, err := os.Lstat(filepath.Join(dir, "base.qcow2")); !errors.Is(err, os.ErrNotExist) {
				t.Fatalf("source link not removed: %v", err)
			}
			data, err := os.ReadFile(disk)
			if successful {
				if !errors.Is(err, os.ErrNotExist) {
					t.Fatalf("successful run retained disk: %v", err)
				}
			} else if err != nil || string(data) != string(contents) {
				t.Fatalf("failed run lost disk evidence: %q, %v", data, err)
			}
		})
	}
}

func TestVariantMetadataAndManifestMustAgree(t *testing.T) {
	tests := []struct {
		name, variant, version, bootloader, manifestVariant string
		valid                                               bool
	}{
		{"standard1.2", "standard", "1.2", "systemd-boot", "standard", true},
		{"standard1.3", "standard", "1.3", "systemd-boot", "standard", true},
		{"mshv1.3", "mshv", "1.3", "efitreeloader-mshv-systemd-boot", "mshv", true},
		{"old mshv metadata", "mshv", "1.2", "efitreeloader-mshv-systemd-boot", "mshv", false},
		{"standard loader for mshv", "mshv", "1.3", "systemd-boot", "mshv", false},
		{"mshv loader for standard", "standard", "1.3", "efitreeloader-mshv-systemd-boot", "standard", false},
		{"mismatched provenance", "mshv", "1.3", "efitreeloader-mshv-systemd-boot", "standard", false},
		{"unknown variant", "other", "1.2", "systemd-boot", "other", false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			raw := strings.Replace(testMetadata, "1.2", tt.version, 1)
			raw = strings.Replace(raw, "systemd-boot", tt.bootloader, 1)
			dir := fixture(t, []byte(raw))
			path := filepath.Join(dir, "manifest.json")
			manifest, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if err = os.WriteFile(path, []byte(strings.Replace(string(manifest), "standard", tt.manifestVariant, 1)), 0600); err != nil {
				t.Fatal(err)
			}
			info, err := readArtifacts(dir, tt.variant)
			if (err == nil) != tt.valid {
				t.Fatalf("valid=%v, got %v", tt.valid, err)
			}
			if tt.valid && info.Variant != tt.variant {
				t.Fatalf("variant not retained: %q", info.Variant)
			}
		})
	}
}

func TestMSHVUsesSharedManualLifecycleAndVariantAssertions(t *testing.T) {
	g := &fakeGuest{disconnect: true}
	if err := service(g, artifactInfo{Variant: "mshv"}); err != nil {
		t.Fatal(err)
	}
	for _, phase := range []string{"baseline", "update-1-staged", "update-1", "rollback", "update-2-staged", "update-2", "update-3-staged", "update-3", "persisted"} {
		if !strings.Contains(strings.Join(g.calls, "\n"), "guest-test.sh "+phase+" mshv ") {
			t.Fatalf("missing MSHV assertion for phase %s: %v", phase, g.calls)
		}
	}
}

func TestLocalKeepsSSHBannerOutOfBootID(t *testing.T) {
	dir := fixture(t, []byte(testMetadata))
	logPath := filepath.Join(dir, "controller.log")
	log, err := os.OpenFile(logPath, os.O_CREATE|os.O_WRONLY, 0600)
	if err != nil {
		t.Fatal(err)
	}
	defer log.Close()
	r := &runner{log: log}
	const bootID = "05c4ce22-b11d-45de-91f3-b3b73e88a4aa"
	const banner = "Welcome to Microsoft Azure Linux 3.0 (x86_64)"
	output, err := r.local(5*time.Second, "sh", "-c", "printf '%s\\n' '"+banner+"' >&2; printf '%s\\n' '"+bootID+"'")
	if err != nil {
		t.Fatal(err)
	}
	if _, err = uuid.Parse(output); err != nil || output != bootID {
		t.Fatalf("stdout boot ID contaminated: %q, %v", output, err)
	}
	data, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), "stderr:\n"+banner+"\n") {
		t.Fatalf("stderr banner missing from controller log: %s", data)
	}
	if !strings.Contains(string(data), "stdout:\n"+bootID+"\n") {
		t.Fatalf("stdout boot ID missing from controller log: %s", data)
	}
}

func TestEvidenceFailureStillSyncsWithoutMaskingErrors(t *testing.T) {
	evidenceErr := errors.New("tar missing")
	syncErr := errors.New("sync SSH disconnected")
	var calls []string
	err := collectAndSync(func() error { calls = append(calls, "evidence"); return evidenceErr }, func() error { calls = append(calls, "sync"); return syncErr })
	if strings.Join(calls, ",") != "evidence,sync" {
		t.Fatalf("unexpected diagnostic order: %v", calls)
	}
	if !errors.Is(err, evidenceErr) || !errors.Is(err, syncErr) {
		t.Fatalf("diagnostic failure masked: %v", err)
	}
}

func TestManualServicingRejectsUnprovisionedOrWrongVolumeAtEveryCheckpoint(t *testing.T) {
	for _, checkpoint := range []string{"update-1", "rollback", "update-2", "update-3", "persisted"} {
		for _, invalidStatus := range []string{
			"servicingState: ab-update-finalized\nabActiveVolume: volume-a\n",
			"servicingState: provisioned\nabActiveVolume: volume-c\n",
		} {
			t.Run(checkpoint+"/"+invalidStatus, func(t *testing.T) {
				g := &fakeGuest{statusCheckpoint: checkpoint, statusOutput: invalidStatus}
				if err := service(g, artifactInfo{Variant: "standard"}); err == nil {
					t.Fatal("accepted incorrect Host Status")
				}
				if !strings.HasSuffix(g.calls[len(g.calls)-1], "/"+checkpoint+"/status") {
					t.Fatalf("continued servicing after invalid status: %v", g.calls)
				}
			})
		}
	}
}
