package generator

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"tridenttools/cmd/mkcosi/gpt"
	"tridenttools/cmd/mkcosi/metadata"

	log "github.com/sirupsen/logrus"
)

// CIH (Code Integrity Host) image detection and metadata population.
//
// CIH is based on Flatcar Linux with a hermetic /usr partition (USR-A) that
// contains most of the OS. The root partition is nearly empty, holding only
// symlinks into /usr. Standard filesystem detection does not work because:
//   - os-release lives in USR-A at lib/os-release (i.e. /usr/lib/os-release)
//   - The root partition has no /etc/os-release or package database
//   - Flatcar-specific partition type GUIDs are used for USR and OEM-CONFIG
//
// CIH images have a static partition layout: all images share the same
// partition numbers, unique partition UUIDs, and partition type GUIDs.
// Update cihRequiredPartitions and cihOptionalPartitions when the CIH
// partition definition changes.

// cihPartitionDef describes one expected partition in a CIH image.
type cihPartitionDef struct {
	Name      string   // GPT partition name (e.g. "USR-A")
	TypeGUIDs []string // Acceptable partition type GUIDs, lowercase. Most
	// entries have exactly one; ROOT and HASH-A/HASH-B carry both the
	// amd64 and arm64 discoverable-partition-spec GUIDs, since a CIH
	// image built for either architecture is valid and the image being
	// inspected here was not necessarily produced on a host of the same
	// architecture (e.g. cross-arch builds, or a build/inspection tool
	// running natively rather than under arch-emulation). Do not key
	// this off runtime.GOARCH -- see cih.go's isCIHImage doc comment.
	UUID string // Unique partition GUID, lowercase; empty means "don't check"
}

// cihRequiredPartitions lists the partitions that must be present (by name and
// type GUID) for an image to be recognized as CIH. USR-A, USR-B, HASH-A, and
// HASH-B have constant partition UUIDs that are additionally verified. Other
// partition UUIDs vary across builds and are not checked.
// HASH-A and HASH-B are optional — images without them are still valid CIH.
var cihRequiredPartitions = []cihPartitionDef{
	{Name: "EFI-SYSTEM", TypeGUIDs: []string{"c12a7328-f81f-11d2-ba4b-00a0c93ec93b"}},
	{Name: "USR-A", TypeGUIDs: []string{"5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6"}, UUID: "7130c94a-213a-4e5a-8e26-6cce9662f132"},
	{Name: "USR-B", TypeGUIDs: []string{"5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6"}, UUID: "e03dd35c-7c2d-4a47-b3fe-27f15780a57c"},
	{Name: "OEM", TypeGUIDs: []string{"0fc63daf-8483-4772-8e79-3d69d8477de4"}},
	{Name: "ROOT", TypeGUIDs: []string{
		string(metadata.PartitionTypeRootAmd64),
		string(metadata.PartitionTypeRootArm64),
	}},
}

// cihOptionalPartitions lists partitions that may or may not be present.
// When present, both name+typeGUID and UUID must match.
var cihOptionalPartitions = []cihPartitionDef{
	{Name: "BIOS-BOOT", TypeGUIDs: []string{"21686148-6449-6e6f-744e-656564454649"}},
	{Name: "OEM-CONFIG", TypeGUIDs: []string{"c95dc21a-df0e-4340-8d7b-26cbfa9a03e0"}},
	{Name: "HASH-A", TypeGUIDs: []string{
		string(metadata.PartitionTypeUsrAmd64Verity),
		string(metadata.PartitionTypeUsrArm64Verity),
	}, UUID: "b736baf1-cdb4-4535-beba-ddaaa30ad7b7"},
	{Name: "HASH-B", TypeGUIDs: []string{
		string(metadata.PartitionTypeUsrAmd64Verity),
		string(metadata.PartitionTypeUsrArm64Verity),
	}, UUID: "35bdf78b-c453-4661-98e6-f834f534ef5b"},
}

// cihMountPointByName maps CIH partition names to their logical mount points.
// Partitions not in this map are still included in the COSI disk regions but
// do not produce an Image entry (e.g. BIOS-BOOT has no filesystem, USR-B is
// the inactive A/B slot, OEM-CONFIG is reserved for first-boot customization).
var cihMountPointByName = map[string]string{
	"EFI-SYSTEM": "/boot",
	"USR-A":      "/usr",
	"OEM":        "/oem",
	"ROOT":       "/",
}

// isCIHImage reports whether the parsed GPT matches the known CIH (Code
// Integrity Host) partition layout. Required partitions must be present by
// name+typeGUID; those with a non-empty UUID are also verified by UUID.
// Optional partitions (HASH-A/HASH-B) are validated when present.
func isCIHImage(parsedGPT *gpt.ParsedGPT) bool {
	type partVal struct {
		typeGUID string
		uuid     string
	}
	// Map partition name -> (typeGUID, UUID) for checking. GPT partition
	// names are unique within a CIH image, so this does not need to be
	// keyed by typeGUID as well.
	partMap := make(map[string]partVal, len(parsedGPT.Partitions))
	for _, p := range parsedGPT.Partitions {
		partMap[p.GetName()] = partVal{
			typeGUID: strings.ToLower(p.PartitionTypeGUID.String()),
			uuid:     strings.ToLower(p.UniquePartitionGUID.String()),
		}
	}

	// matchesTypeGUID reports whether actual is one of the acceptable type
	// GUIDs for a partition definition. Some logical partitions (ROOT,
	// HASH-A, HASH-B) accept either the amd64 or arm64 discoverable-
	// partition-spec GUID: a CIH image is valid for either architecture,
	// and the architecture of the image under inspection is not assumed
	// to match the architecture of whatever is running this check.
	matchesTypeGUID := func(def cihPartitionDef, actual string) bool {
		for _, want := range def.TypeGUIDs {
			if actual == want {
				return true
			}
		}
		return false
	}

	// All required partitions must be present with matching name+typeGUID.
	// Those with a specified UUID must also match.
	for _, req := range cihRequiredPartitions {
		actual, found := partMap[req.Name]
		if !found || !matchesTypeGUID(req, actual.typeGUID) {
			return false
		}
		if req.UUID != "" && actual.uuid != req.UUID {
			return false
		}
	}

	// Optional partitions: if present, their type GUID and UUID must match.
	for _, opt := range cihOptionalPartitions {
		actual, found := partMap[opt.Name]
		if !found {
			continue
		}
		if !matchesTypeGUID(opt, actual.typeGUID) {
			continue
		}
		if opt.UUID != "" && actual.uuid != opt.UUID {
			return false
		}
	}

	return true
}

// populateCIHFilesystemMetadata fills COSI metadata for a CIH image.
// It uses partition names (rather than type GUIDs) to determine mount points
// and extracts os-release from the USR-A partition instead of root.
// Image entries are NOT created here — they are built by the caller after
// compression.
func populateCIHFilesystemMetadata(cosiMeta *metadata.MetadataJson, partInfos []partitionInfo, tmpDir string) error {
	mountTmpDir := filepath.Join(tmpDir, "mounts")
	if err := os.MkdirAll(mountTmpDir, 0755); err != nil {
		return fmt.Errorf("failed to create mounts directory: %w", err)
	}

	// Initialize empty slice to avoid null in JSON output when no packages are found.
	cosiMeta.OsPackages = make([]metadata.OsPackage, 0)

	var usrAMountPath string
	var espMountPath string
	var espMountPoint string

	// Build a lookup from partition name to index for verity pairing.
	nameToIdx := make(map[string]int, len(partInfos))
	for i := range partInfos {
		nameToIdx[partInfos[i].entry.GetName()] = i
	}

	for i := range partInfos {
		pi := &partInfos[i]
		partName := pi.entry.GetName()

		// Determine mount point from the CIH partition name table.
		mountPoint, known := cihMountPointByName[partName]
		if !known {
			log.WithFields(log.Fields{
				"partition": pi.partNumber,
				"name":      partName,
			}).Debug("CIH: skipping partition with no mount point mapping")
			continue
		}

		// Get filesystem type and UUID via blkid on the raw file.
		fsType, fsUuid, err := getFsData(pi.rawPath)
		if err != nil {
			log.WithError(err).WithFields(log.Fields{
				"partition": pi.partNumber,
				"name":      partName,
			}).Warn("CIH: could not get filesystem data, skipping")
			continue
		}
		pi.fsType = fsType
		pi.fsUuid = fsUuid
		pi.mountPoint = mountPoint

		// Mount the raw partition read-only.
		mountPath := filepath.Join(mountTmpDir, fmt.Sprintf("part%d", pi.partNumber))
		if err := os.MkdirAll(mountPath, 0755); err != nil {
			return fmt.Errorf("failed to create mount point: %w", err)
		}

		if err := exec.Command("mount", "-o", "loop,ro", pi.rawPath, mountPath).Run(); err != nil {
			log.WithError(err).WithFields(log.Fields{
				"partition": pi.partNumber,
				"name":      partName,
			}).Warn("CIH: could not mount partition, skipping")
			continue
		}
		defer exec.Command("umount", mountPath).Run()

		log.WithFields(log.Fields{
			"partition":  pi.partNumber,
			"name":       partName,
			"mountPoint": mountPoint,
			"fsType":     fsType,
		}).Info("CIH: processed partition")

		// Track special mount paths for later metadata extraction.
		switch partName {
		case "USR-A":
			usrAMountPath = mountPath
		case "EFI-SYSTEM":
			espMountPath = mountPath
			espMountPoint = mountPoint
		}
	}

	// Extract os-release from USR-A.
	// In CIH, the USR-A partition is mounted at /usr, so os-release is at
	// <mount>/lib/os-release (i.e. /usr/lib/os-release on the running system).
	if usrAMountPath != "" {
		osReleasePath := filepath.Join(usrAMountPath, "lib", "os-release")
		data, err := os.ReadFile(osReleasePath)
		if err != nil {
			log.WithError(err).Warn("CIH: could not read os-release from USR-A")
		} else {
			cosiMeta.OsRelease = string(data)
			log.Info("CIH: extracted os-release from USR-A")
		}
	} else {
		log.Warn("CIH: USR-A partition not mounted, cannot extract os-release")
	}

	// Try to extract installed packages. CIH images typically do not have a
	// traditional RPM/DPKG database, so this may return nothing.
	if usrAMountPath != "" {
		packages, err := extractPackages(usrAMountPath)
		if err != nil {
			log.Debug("CIH: no package database found (expected for hermetic /usr images)")
		} else {
			log.WithField("count", len(packages)).Info("CIH: extracted package list")
			cosiMeta.OsPackages = packages
		}
	}

	// Detect bootloader. CIH uses systemd-boot with UKI. This must happen
	// before verity extraction because the USR root hash is embedded in the
	// UKI command line (usrhash=<hex>).
	var ukiEntries []metadata.SystemDBootEntry
	if espMountPath != "" {
		ukiEntries = findUkiEntries(espMountPath, espMountPoint)
		if len(ukiEntries) > 0 {
			log.WithField("count", len(ukiEntries)).Info("CIH: found systemd-boot with UKI entries")
			cosiMeta.Bootloader = metadata.Bootloader{
				Type: metadata.BootloaderTypeSystemDBoot,
				SystemDBoot: &metadata.SystemDBoot{
					Entries: ukiEntries,
				},
			}
		} else if checkGrubPresence(espMountPath) {
			cosiMeta.Bootloader = metadata.Bootloader{
				Type: metadata.BootloaderTypeGrub,
			}
		}
	}

	if cosiMeta.Bootloader.Type == "" {
		return fmt.Errorf("no supported bootloader found in CIH image")
	}

	// If HASH-A is present, pair it with USR-A and extract the dm-verity
	// root hash from the UKI command line. The root hash is NOT stored on
	// the hash device itself; it is passed via the "usrhash=" kernel
	// parameter embedded in the UKI .cmdline section.
	if hashAIdx, ok := nameToIdx["HASH-A"]; ok {
		if usrAIdx, ok := nameToIdx["USR-A"]; ok {
			log.Info("CIH: HASH-A partition found, extracting dm-verity root hash from UKI cmdline")

			roothash := extractUsrhashFromUKIEntries(ukiEntries)
			if roothash == "" {
				return fmt.Errorf("HASH-A partition present but usrhash= not found in any UKI command line")
			}

			log.WithField("roothash", roothash).Info("CIH: extracted dm-verity root hash for USR-A")
			partInfos[usrAIdx].verityHashPartIdx = hashAIdx
			partInfos[usrAIdx].verityRoothash = roothash
		}
	}

	return nil
}

// extractUsrhashFromUKIEntries searches the UKI boot entries for a
// "usrhash=<hex>" kernel command-line parameter and returns the hash value.
// Addon cmdlines are already folded into the entry's Cmdline by the generator.
// Returns an empty string if not found.
func extractUsrhashFromUKIEntries(entries []metadata.SystemDBootEntry) string {
	for _, entry := range entries {
		for _, field := range strings.Fields(entry.Cmdline) {
			if after, found := strings.CutPrefix(field, "usrhash="); found {
				return after
			}
		}
	}
	return ""
}
