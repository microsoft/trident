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
// Update cihExpectedPartitions when the CIH partition definition changes.

// cihPartitionDef describes one expected partition in a CIH image.
type cihPartitionDef struct {
	Name     string // GPT partition name (e.g. "USR-A")
	TypeGUID string // Partition type GUID, lowercase
	UUID     string // Unique partition GUID, lowercase; empty means "don't check"
}

// cihRequiredPartitions lists the partitions that must be present (by name and
// type GUID) for an image to be recognized as CIH. USR-A, USR-B, HASH-A, and
// HASH-B have constant partition UUIDs that are additionally verified. Other
// partition UUIDs vary across builds and are not checked.
// HASH-A and HASH-B are optional — images without them are still valid CIH.
var cihRequiredPartitions = []cihPartitionDef{
	{Name: "EFI-SYSTEM", TypeGUID: "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"},
	{Name: "BIOS-BOOT", TypeGUID: "21686148-6449-6e6f-744e-656564454649"},
	{Name: "USR-A", TypeGUID: "5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6", UUID: "7130c94a-213a-4e5a-8e26-6cce9662f132"},
	{Name: "USR-B", TypeGUID: "5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6", UUID: "e03dd35c-7c2d-4a47-b3fe-27f15780a57c"},
	{Name: "OEM", TypeGUID: "0fc63daf-8483-4772-8e79-3d69d8477de4"},
	{Name: "OEM-CONFIG", TypeGUID: "c95dc21a-df0e-4340-8d7b-26cbfa9a03e0"},
	{Name: "ROOT", TypeGUID: "4f68bce3-e8cd-4db1-96e7-fbcaf984b709"},
}

// cihOptionalPartitions lists partitions that may or may not be present.
// When present, both name+typeGUID and UUID must match.
var cihOptionalPartitions = []cihPartitionDef{
	{Name: "HASH-A", TypeGUID: "77ff5f63-e7b6-4633-acf4-1565b864c0e6", UUID: "b736baf1-cdb4-4535-beba-ddaaa30ad7b7"},
	{Name: "HASH-B", TypeGUID: "77ff5f63-e7b6-4633-acf4-1565b864c0e6", UUID: "35bdf78b-c453-4661-98e6-f834f534ef5b"},
}

// cihMountPointByName maps CIH partition names to their logical mount points.
// Partitions not in this map are still included in the COSI disk regions but
// do not produce an Image entry (e.g. BIOS-BOOT has no filesystem, USR-B is
// the inactive A/B slot, OEM-CONFIG is reserved for first-boot customization).
var cihMountPointByName = map[string]string{
	"EFI-SYSTEM": "/boot/efi",
	"USR-A":      "/usr",
	"OEM":        "/oem",
	"ROOT":       "/",
}

// isCIHImage reports whether the parsed GPT matches the known CIH (Code
// Integrity Host) partition layout. Required partitions must be present by
// name+typeGUID; those with a non-empty UUID are also verified by UUID.
// Optional partitions (HASH-A/HASH-B) are validated when present.
func isCIHImage(parsedGPT *gpt.ParsedGPT) bool {
	type partKey struct {
		name     string
		typeGUID string
	}
	// Map name+typeGUID -> partition UUID for checking.
	partMap := make(map[partKey]string, len(parsedGPT.Partitions))
	for _, p := range parsedGPT.Partitions {
		partMap[partKey{
			name:     p.GetName(),
			typeGUID: strings.ToLower(p.PartitionTypeGUID.String()),
		}] = strings.ToLower(p.UniquePartitionGUID.String())
	}

	// All required partitions must be present with matching name+typeGUID.
	// Those with a specified UUID must also match.
	for _, req := range cihRequiredPartitions {
		key := partKey{name: req.Name, typeGUID: req.TypeGUID}
		actualUUID, found := partMap[key]
		if !found {
			return false
		}
		if req.UUID != "" && actualUUID != req.UUID {
			return false
		}
	}

	// Optional partitions: if present, their UUID must match.
	for _, opt := range cihOptionalPartitions {
		key := partKey{name: opt.Name, typeGUID: opt.TypeGUID}
		actualUUID, found := partMap[key]
		if found && opt.UUID != "" && actualUUID != opt.UUID {
			return false
		}
	}

	return true
}

// populateCIHFilesystemMetadata fills COSI metadata for a CIH image.
// It uses partition names (rather than type GUIDs) to determine mount points
// and extracts os-release from the USR-A partition instead of root.
func populateCIHFilesystemMetadata(cosiMeta *metadata.MetadataJson, partInfos []partitionInfo, tmpDir string) error {
	mountTmpDir := filepath.Join(tmpDir, "mounts")
	if err := os.MkdirAll(mountTmpDir, 0755); err != nil {
		return fmt.Errorf("failed to create mounts directory: %w", err)
	}

	var usrAMountPath string
	var espMountPath string
	var espMountPoint string
	var usrAImageIdx int = -1

	// Build a lookup from partition name to partitionInfo for HASH-A.
	partByName := make(map[string]*partitionInfo, len(partInfos))
	for i := range partInfos {
		partByName[partInfos[i].entry.GetName()] = &partInfos[i]
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

		// Decompress the partition image.
		decompressedPath := filepath.Join(tmpDir, fmt.Sprintf("partition-%d.raw", pi.partNumber))
		if err := decompressFile(pi.imageFile.SourceFile, decompressedPath); err != nil {
			return fmt.Errorf("failed to decompress partition %d (%s): %w", pi.partNumber, partName, err)
		}

		// Get filesystem type and UUID via blkid.
		fsType, fsUuid, err := getFsData(decompressedPath)
		if err != nil {
			os.Remove(decompressedPath)
			log.WithError(err).WithFields(log.Fields{
				"partition": pi.partNumber,
				"name":      partName,
			}).Warn("CIH: could not get filesystem data, skipping")
			continue
		}
		pi.fsType = fsType
		pi.fsUuid = fsUuid
		pi.mountPoint = mountPoint

		// Mount the partition read-only.
		mountPath := filepath.Join(mountTmpDir, fmt.Sprintf("part%d", pi.partNumber))
		if err := os.MkdirAll(mountPath, 0755); err != nil {
			os.Remove(decompressedPath)
			return fmt.Errorf("failed to create mount point: %w", err)
		}

		if err := exec.Command("mount", "-o", "loop,ro", decompressedPath, mountPath).Run(); err != nil {
			os.Remove(decompressedPath)
			log.WithError(err).WithFields(log.Fields{
				"partition": pi.partNumber,
				"name":      partName,
			}).Warn("CIH: could not mount partition, skipping")
			continue
		}

		defer func(mp, dp string) {
			exec.Command("umount", mp).Run()
			os.Remove(dp)
		}(mountPath, decompressedPath)

		// Create the Image entry.
		partType := uuidToPartitionType(pi.entry.PartitionTypeGUID)
		imageIdx := len(cosiMeta.Images)
		cosiMeta.Images = append(cosiMeta.Images, metadata.Image{
			Image:      *pi.imageFile,
			MountPoint: mountPoint,
			FsType:     fsType,
			FsUuid:     fsUuid,
			PartType:   partType,
			Verity:     nil,
		})

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
			usrAImageIdx = imageIdx
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

	// If HASH-A is present, populate dm-verity metadata for USR-A.
	if hashAPart, ok := partByName["HASH-A"]; ok && usrAImageIdx >= 0 {
		log.Info("CIH: HASH-A partition found, extracting dm-verity root hash")

		// Decompress both USR-A and HASH-A to extract the root hash.
		usrADecompressed := filepath.Join(tmpDir, fmt.Sprintf("verity-data-%d.raw", partByName["USR-A"].partNumber))
		if err := decompressFile(partByName["USR-A"].imageFile.SourceFile, usrADecompressed); err != nil {
			return fmt.Errorf("failed to decompress USR-A for verity: %w", err)
		}
		defer os.Remove(usrADecompressed)

		hashADecompressed := filepath.Join(tmpDir, fmt.Sprintf("verity-hash-%d.raw", hashAPart.partNumber))
		if err := decompressFile(hashAPart.imageFile.SourceFile, hashADecompressed); err != nil {
			return fmt.Errorf("failed to decompress HASH-A for verity: %w", err)
		}
		defer os.Remove(hashADecompressed)

		roothash, err := extractVerityRoothash(usrADecompressed, hashADecompressed)
		if err != nil {
			return fmt.Errorf("failed to extract dm-verity root hash: %w", err)
		}

		log.WithField("roothash", roothash).Info("CIH: extracted dm-verity root hash for USR-A")
		cosiMeta.Images[usrAImageIdx].Verity = &metadata.Verity{
			Image:    *hashAPart.imageFile,
			Roothash: roothash,
		}
	}

	// Detect bootloader. CIH uses systemd-boot with UKI.
	if espMountPath != "" {
		ukiEntries := findUkiEntries(espMountPath, espMountPoint)
		if len(ukiEntries) > 0 {
			log.WithField("count", len(ukiEntries)).Info("CIH: found systemd-boot with UKI entries")
			cosiMeta.Bootloader = metadata.Bootloader{
				Type: metadata.BootloaderTypeSystemDBoot,
				SystemDBoot: &metadata.SystemDBoot{
					Entries: ukiEntries,
				},
			}
			return nil
		}

		if checkGrubPresence(espMountPath) {
			cosiMeta.Bootloader = metadata.Bootloader{
				Type: metadata.BootloaderTypeGrub,
			}
			return nil
		}
	}

	return fmt.Errorf("no supported bootloader found in CIH image")
}

// extractVerityRoothash extracts the dm-verity root hash from a data device
// and its hash device using veritysetup dump.
func extractVerityRoothash(dataDevice string, hashDevice string) (string, error) {
	cmd := exec.Command("veritysetup", "dump", dataDevice, hashDevice)
	output, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("veritysetup dump failed: %w", err)
	}

	for _, line := range strings.Split(string(output), "\n") {
		if after, found := strings.CutPrefix(line, "Root hash:"); found {
			roothash := strings.TrimSpace(after)
			if roothash != "" {
				return roothash, nil
			}
		}
	}

	return "", fmt.Errorf("root hash not found in veritysetup dump output")
}
