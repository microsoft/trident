// Package acl exercises manual adoption and A/B servicing of synthetic ACL guests.
package acl

import (
	"archive/tar"
	"bytes"
	"crypto/sha256"
	"crypto/sha512"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/microsoft/storm"
	metadata "tridenttools/cmd/mkcosi/metadata"
)

const guestDir = "/var/lib/trident/synthetic-acl"

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

func updateConfig(info artifactInfo) string {
	return fmt.Sprintf("image:\n  url: file://%s/update.cosi\n  sha384: %s\ninternalParams:\n  forceAbUpdate: true\n  noTransition: true\nos:\n  uefiFallback: conservative\n", guestDir, info.MetadataSHA384)
}
