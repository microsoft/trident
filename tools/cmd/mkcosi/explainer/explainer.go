// Package explainer provides a developer-friendly explanation of a COSI file's
// contents by correlating the raw tar layout with the parsed metadata.
package explainer

import (
	"archive/tar"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strings"

	"tridenttools/cmd/mkcosi/metadata"

	"github.com/dustin/go-humanize"
	"github.com/fatih/color"
)

const tarBlock = 512

// tarEntry records the raw position and size of a single tar member.
type tarEntry struct {
	Index        int
	Name         string
	HeaderOffset int64
	DataOffset   int64
	Size         int64 // logical (unpadded) size
	PaddedSize   int64 // size rounded up to 512-byte boundary
	TypeFlag     byte
}

// ExplainCosiFile opens the file at path and prints a developer-friendly
// explanation of the COSI file layout and metadata. It is best-effort: any
// non-fatal inconsistency is reported inline rather than aborting.
func ExplainCosiFile(path string) error {
	f, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open: %w", err)
	}
	defer f.Close()

	stat, err := f.Stat()
	if err != nil {
		return fmt.Errorf("stat: %w", err)
	}
	fileSize := stat.Size()

	// ── Phase 1: scan the raw tar to collect entries ──────────────────────
	entries, endMarkerOffset, err := scanTar(f, fileSize)
	if err != nil {
		return fmt.Errorf("scan tar: %w", err)
	}

	// ── Phase 2: read metadata.json from the tar ─────────────────────────
	meta, metaRaw, metaErr := readMetadata(f)

	// ── Phase 3: render ──────────────────────────────────────────────────
	bold := color.New(color.Bold)
	header := color.New(color.Bold, color.FgCyan)
	warn := color.New(color.FgYellow)
	errC := color.New(color.FgRed, color.Bold)
	good := color.New(color.FgGreen)
	dim := color.New(color.Faint)

	// — File overview —
	header.Println("═══════════════════════════════════════════════════════════")
	header.Printf("  COSI File: %s\n", path)
	header.Println("═══════════════════════════════════════════════════════════")
	fmt.Println()
	bold.Print("  File size:       ")
	fmt.Printf("%s (%d bytes)\n", humanize.IBytes(uint64(fileSize)), fileSize)
	bold.Print("  Tar entries:     ")
	fmt.Printf("%d\n", len(entries))

	if endMarkerOffset >= 0 {
		bold.Print("  End-of-archive:  ")
		fmt.Printf("0x%08x (%d)\n", endMarkerOffset, endMarkerOffset)

		trailingStart := endMarkerOffset + 2*tarBlock
		if trailingStart < fileSize {
			trailingLen := fileSize - trailingStart
			bold.Print("  Trailing data:   ")
			warn.Printf("%s (%d bytes) after end-of-archive marker\n",
				humanize.IBytes(uint64(trailingLen)), trailingLen)
		}
	} else {
		bold.Print("  End-of-archive:  ")
		warn.Println("NOT FOUND (malformed tar)")
	}

	// — COSI marker check —
	fmt.Println()
	if len(entries) > 0 && entries[0].Name == "cosi-marker" {
		good.Println("  ✓ cosi-marker is the first entry")
		if entries[0].Size != 0 {
			warn.Printf("  ⚠ cosi-marker should be 0 bytes, got %d\n", entries[0].Size)
		}
	} else {
		errC.Println("  ✗ cosi-marker is NOT the first entry (not a valid COSI file)")
	}

	// — Tar entry table —
	fmt.Println()
	header.Println("───────────────────────────────────────────────────────────")
	header.Println("  Tar Layout")
	header.Println("───────────────────────────────────────────────────────────")
	fmt.Println()

	nameWidth := 50
	fmt.Printf("  %-4s  %-12s  %-12s  %-8s  %-*s  %s\n",
		"#", "Header", "Data", "Size", nameWidth, "Name", "Type")
	fmt.Printf("  %s  %s  %s  %s  %s  %s\n",
		strings.Repeat("─", 4),
		strings.Repeat("─", 12),
		strings.Repeat("─", 12),
		strings.Repeat("─", 8),
		strings.Repeat("─", nameWidth),
		strings.Repeat("─", 10))

	for _, e := range entries {
		typeDesc := tarTypeDesc(e.TypeFlag)
		nameDisplay := e.Name
		if len(nameDisplay) > nameWidth {
			nameDisplay = nameDisplay[:nameWidth-3] + "..."
		}
		sizeStr := humanize.IBytes(uint64(e.Size))

		fmt.Printf("  %-4d  0x%08x    0x%08x    %-8s  %-*s  %s\n",
			e.Index,
			e.HeaderOffset,
			e.DataOffset,
			sizeStr,
			nameWidth,
			nameDisplay,
			typeDesc)
	}

	// — Metadata section —
	fmt.Println()
	header.Println("───────────────────────────────────────────────────────────")
	header.Println("  Metadata (metadata.json)")
	header.Println("───────────────────────────────────────────────────────────")
	fmt.Println()

	if metaErr != nil {
		errC.Printf("  ✗ Failed to read metadata.json: %v\n", metaErr)
		return nil // best-effort: we still printed the tar layout
	}

	if meta == nil {
		errC.Println("  ✗ metadata.json not found in the tar")
		return nil
	}

	bold.Print("  COSI version:    ")
	fmt.Println(meta.Version)
	bold.Print("  OS arch:         ")
	fmt.Println(string(meta.OsArch))
	bold.Print("  Bootloader:      ")
	fmt.Println(string(meta.Bootloader.Type))
	bold.Print("  Packages:        ")
	fmt.Printf("%d installed\n", len(meta.OsPackages))

	if meta.Id != "" {
		bold.Print("  ID:              ")
		fmt.Println(meta.Id)
	}

	if meta.Compression != nil {
		bold.Print("  Compression:     ")
		fmt.Printf("maxWindowLog=%d (window size %s)\n",
			meta.Compression.MaxWindowLog,
			humanize.IBytes(uint64(1)<<meta.Compression.MaxWindowLog))
	}

	bold.Print("  JSON size:       ")
	fmt.Printf("%s (%d bytes)\n", humanize.IBytes(uint64(len(metaRaw))), len(metaRaw))

	// — os-release —
	if meta.OsRelease != "" {
		fmt.Println()
		bold.Println("  os-release:")
		for _, line := range strings.Split(meta.OsRelease, "\n") {
			if line = strings.TrimSpace(line); line != "" {
				dim.Printf("    %s\n", line)
			}
		}
	}

	// — Disk info —
	if meta.Disk != nil {
		fmt.Println()
		header.Println("───────────────────────────────────────────────────────────")
		header.Println("  Disk Layout")
		header.Println("───────────────────────────────────────────────────────────")
		fmt.Println()

		bold.Print("  Disk type:       ")
		fmt.Println(string(meta.Disk.Type))
		bold.Print("  Disk size:       ")
		fmt.Printf("%s (%d bytes)\n", humanize.IBytes(meta.Disk.Size), meta.Disk.Size)
		bold.Print("  LBA size:        ")
		fmt.Printf("%d bytes\n", meta.Disk.LbaSize)
		bold.Print("  GPT regions:     ")
		fmt.Printf("%d\n", len(meta.Disk.GptRegions))

		if len(meta.Disk.GptRegions) > 0 {
			fmt.Println()
			printGptRegions(meta, entries, bold, warn, good, dim)
		}
	}

	// — Filesystem images —
	fmt.Println()
	header.Println("───────────────────────────────────────────────────────────")
	header.Println("  Filesystem Images")
	header.Println("───────────────────────────────────────────────────────────")
	fmt.Println()

	if len(meta.Images) == 0 {
		warn.Println("  (no filesystem images)")
	}

	// Build a lookup from tar entry name to entry for cross-referencing.
	tarByName := make(map[string]*tarEntry, len(entries))
	for i := range entries {
		tarByName[entries[i].Name] = &entries[i]
	}

	for i, img := range meta.Images {
		printFilesystemImage(i, &img, tarByName, bold, warn, errC, good, dim)
	}

	// — Bootloader details —
	if meta.Bootloader.Type == metadata.BootloaderTypeSystemDBoot && meta.Bootloader.SystemDBoot != nil {
		fmt.Println()
		header.Println("───────────────────────────────────────────────────────────")
		header.Println("  Bootloader (systemd-boot)")
		header.Println("───────────────────────────────────────────────────────────")
		fmt.Println()

		for i, entry := range meta.Bootloader.SystemDBoot.Entries {
			bold.Printf("  Entry %d:\n", i+1)
			fmt.Printf("    Type:     %s\n", entry.Type)
			fmt.Printf("    Path:     %s\n", entry.Path)
			fmt.Printf("    Kernel:   %s\n", entry.Kernel)
			if entry.Cmdline != "" {
				cmdline := entry.Cmdline
				if len(cmdline) > 100 {
					cmdline = cmdline[:97] + "..."
				}
				fmt.Printf("    Cmdline:  %s\n", cmdline)
			}
		}
	}

	// — Cross-reference: orphan tar entries (in tar but not in metadata) —
	fmt.Println()
	header.Println("───────────────────────────────────────────────────────────")
	header.Println("  Cross-Reference")
	header.Println("───────────────────────────────────────────────────────────")
	fmt.Println()

	referenced := referencedPaths(meta)
	var orphans []string
	for _, e := range entries {
		if e.Name == "cosi-marker" || e.Name == "metadata.json" {
			continue
		}
		if _, ok := referenced[e.Name]; !ok {
			orphans = append(orphans, e.Name)
		}
	}

	if len(orphans) == 0 {
		good.Println("  ✓ All tar image entries are referenced by metadata")
	} else {
		warn.Println("  ⚠ Tar entries NOT referenced by metadata:")
		for _, name := range orphans {
			warn.Printf("      • %s\n", name)
		}
	}

	// Check for metadata references that are missing from the tar.
	var missing []string
	for refPath := range referenced {
		if _, ok := tarByName[refPath]; !ok {
			missing = append(missing, refPath)
		}
	}

	if len(missing) == 0 {
		good.Println("  ✓ All metadata image paths found in tar")
	} else {
		errC.Println("  ✗ Metadata references NOT found in tar:")
		for _, name := range missing {
			errC.Printf("      • %s\n", name)
		}
	}

	// Check compressed size consistency.
	var sizeIssues []string
	for refPath, expectedSize := range referenced {
		te, ok := tarByName[refPath]
		if !ok {
			continue
		}
		if uint64(te.Size) != expectedSize {
			sizeIssues = append(sizeIssues, fmt.Sprintf(
				"%s: tar size %d vs metadata compressedSize %d",
				refPath, te.Size, expectedSize))
		}
	}

	if len(sizeIssues) == 0 {
		good.Println("  ✓ Compressed sizes match between tar and metadata")
	} else {
		errC.Println("  ✗ Compressed size mismatches:")
		for _, issue := range sizeIssues {
			errC.Printf("      • %s\n", issue)
		}
	}

	fmt.Println()
	return nil
}

// scanTar walks the tar archive and returns every entry with raw offsets.
// It also returns the offset of the end-of-archive marker, or -1 if not found.
func scanTar(f *os.File, fileSize int64) ([]tarEntry, int64, error) {
	if _, err := f.Seek(0, io.SeekStart); err != nil {
		return nil, -1, err
	}

	var entries []tarEntry
	tr := tar.NewReader(f)
	index := 0

	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			// Best-effort: return what we have so far.
			break
		}

		// Calculate the header offset. The data starts immediately after the
		// header (512 bytes). We can recover the header offset from the
		// current position of the underlying reader and the data size.
		//
		// After tar.Reader.Next() the reader is positioned at the start of the
		// entry data. We can get the current position:
		currentPos, seekErr := f.Seek(0, io.SeekCurrent)
		if seekErr != nil {
			return nil, -1, fmt.Errorf("seek: %w", seekErr)
		}

		dataOffset := currentPos
		headerOffset := dataOffset - tarBlock
		// For PAX headers, the header might start earlier due to extended
		// headers. The simple heuristic here gives the position of the
		// _last_ 512-byte header block for this entry.
		padded := roundUp(hdr.Size, tarBlock)

		entries = append(entries, tarEntry{
			Index:        index,
			Name:         hdr.Name,
			HeaderOffset: headerOffset,
			DataOffset:   dataOffset,
			Size:         hdr.Size,
			PaddedSize:   padded,
			TypeFlag:     hdr.Typeflag,
		})
		index++
	}

	// Determine end-of-archive marker position.
	endMarker := int64(-1)
	if len(entries) > 0 {
		last := entries[len(entries)-1]
		candidate := last.DataOffset + roundUp(last.Size, tarBlock)
		// Check if the two zero blocks exist at that position.
		if candidate+2*tarBlock <= fileSize {
			buf := make([]byte, 2*tarBlock)
			if _, err := f.ReadAt(buf, candidate); err == nil {
				allZero := true
				for _, b := range buf {
					if b != 0 {
						allZero = false
						break
					}
				}
				if allZero {
					endMarker = candidate
				}
			}
		}
	}

	return entries, endMarker, nil
}

// readMetadata seeks back to the start and reads metadata.json from the tar.
func readMetadata(f *os.File) (*metadata.MetadataJson, []byte, error) {
	if _, err := f.Seek(0, io.SeekStart); err != nil {
		return nil, nil, err
	}

	tr := tar.NewReader(f)
	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			return nil, nil, nil // not found
		}
		if err != nil {
			return nil, nil, fmt.Errorf("reading tar: %w", err)
		}

		if hdr.Name == "metadata.json" {
			raw, err := io.ReadAll(tr)
			if err != nil {
				return nil, nil, fmt.Errorf("reading metadata.json: %w", err)
			}

			var m metadata.MetadataJson
			if err := json.Unmarshal(raw, &m); err != nil {
				return nil, raw, fmt.Errorf("parsing metadata.json: %w", err)
			}
			return &m, raw, nil
		}
	}
}

// printGptRegions prints the GPT disk regions table with cross-references.
func printGptRegions(
	meta *metadata.MetadataJson,
	entries []tarEntry,
	bold, warn, good, dim *color.Color,
) {
	tarByName := make(map[string]*tarEntry, len(entries))
	for i := range entries {
		tarByName[entries[i].Name] = &entries[i]
	}

	// Build a lookup from image path to filesystem info.
	fsByPath := make(map[string]*metadata.Image, len(meta.Images))
	for i := range meta.Images {
		fsByPath[meta.Images[i].Image.Path] = &meta.Images[i]
	}

	fmt.Printf("  %-4s  %-14s  %-*s  %-12s  %-12s  %s\n",
		"#", "Region Type", 50, "Image Path", "Compressed", "Uncompressed", "Filesystem")
	fmt.Printf("  %s  %s  %s  %s  %s  %s\n",
		strings.Repeat("─", 4),
		strings.Repeat("─", 14),
		strings.Repeat("─", 50),
		strings.Repeat("─", 12),
		strings.Repeat("─", 12),
		strings.Repeat("─", 30))

	for i, region := range meta.Disk.GptRegions {
		pathDisplay := region.Image.Path
		if len(pathDisplay) > 50 {
			pathDisplay = pathDisplay[:47] + "..."
		}

		compressed := humanize.IBytes(region.Image.CompressedSize)
		uncompressed := humanize.IBytes(region.Image.UncompressedSize)

		fsInfo := ""
		if fs, ok := fsByPath[region.Image.Path]; ok {
			fsInfo = fmt.Sprintf("%s @ %s (%s)",
				fs.FsType, fs.MountPoint, partTypeName(fs.PartType))
			if fs.Verity != nil {
				fsInfo += " [verity]"
			}
		} else if region.Type == metadata.RegionTypePartition {
			fsInfo = dim.Sprint("(no filesystem mapping)")
		}
		regionLabel := string(region.Type)
		if region.Number != nil {
			regionLabel = fmt.Sprintf("%s #%d", region.Type, *region.Number)
		}

		// Check tar presence.
		marker := " "
		if _, ok := tarByName[region.Image.Path]; !ok {
			marker = warn.Sprint("⚠")
		}

		fmt.Printf("  %-4d  %-14s  %-50s  %-12s  %-12s  %s %s\n",
			i, regionLabel, pathDisplay, compressed, uncompressed, fsInfo, marker)
	}
}

// printFilesystemImage prints details about a single filesystem image.
func printFilesystemImage(
	idx int,
	img *metadata.Image,
	tarByName map[string]*tarEntry,
	bold, warn, errC, good, dim *color.Color,
) {
	bold.Printf("  Filesystem %d:\n", idx+1)
	fmt.Printf("    Mount point:      %s\n", img.MountPoint)
	fmt.Printf("    FS type:          %s\n", img.FsType)
	fmt.Printf("    FS UUID:          %s\n", img.FsUuid)
	fmt.Printf("    Partition type:   %s (%s)\n",
		string(img.PartType), partTypeName(img.PartType))
	fmt.Printf("    Image path:       %s\n", img.Image.Path)
	fmt.Printf("    Compressed:       %s (%d bytes)\n",
		humanize.IBytes(img.Image.CompressedSize), img.Image.CompressedSize)
	fmt.Printf("    Uncompressed:     %s (%d bytes)\n",
		humanize.IBytes(img.Image.UncompressedSize), img.Image.UncompressedSize)

	if img.Image.CompressedSize > 0 {
		ratio := float64(img.Image.UncompressedSize) / float64(img.Image.CompressedSize)
		fmt.Printf("    Compression:      %.1fx ratio\n", ratio)
	}

	if img.Image.Sha384 != "" {
		sha := img.Image.Sha384
		if len(sha) > 24 {
			sha = sha[:12] + "..." + sha[len(sha)-12:]
		}
		fmt.Printf("    SHA-384:          %s\n", sha)
	}

	// Cross-reference with tar.
	if te, ok := tarByName[img.Image.Path]; ok {
		if uint64(te.Size) == img.Image.CompressedSize {
			good.Printf("    Tar:              ✓ found at offset 0x%08x, size matches\n", te.HeaderOffset)
		} else {
			warn.Printf("    Tar:              ⚠ found at offset 0x%08x, size mismatch (tar=%d, meta=%d)\n",
				te.HeaderOffset, te.Size, img.Image.CompressedSize)
		}
	} else {
		errC.Printf("    Tar:              ✗ NOT found in tar archive\n")
	}

	if img.Verity != nil {
		fmt.Printf("    Verity:\n")
		fmt.Printf("      Hash image:     %s\n", img.Verity.Image.Path)
		fmt.Printf("      Compressed:     %s (%d bytes)\n",
			humanize.IBytes(img.Verity.Image.CompressedSize), img.Verity.Image.CompressedSize)
		fmt.Printf("      Uncompressed:   %s (%d bytes)\n",
			humanize.IBytes(img.Verity.Image.UncompressedSize), img.Verity.Image.UncompressedSize)

		roothash := img.Verity.Roothash
		if len(roothash) > 32 {
			roothash = roothash[:32] + "..."
		}
		fmt.Printf("      Root hash:      %s\n", roothash)

		if te, ok := tarByName[img.Verity.Image.Path]; ok {
			if uint64(te.Size) == img.Verity.Image.CompressedSize {
				good.Printf("      Tar:            ✓ found at offset 0x%08x\n", te.HeaderOffset)
			} else {
				warn.Printf("      Tar:            ⚠ size mismatch (tar=%d, meta=%d)\n",
					te.Size, img.Verity.Image.CompressedSize)
			}
		} else {
			errC.Printf("      Tar:            ✗ NOT found in tar archive\n")
		}
	}

	fmt.Println()
}

// referencedPaths returns a set of all image paths referenced by the metadata
// mapped to their expected compressedSize.
func referencedPaths(meta *metadata.MetadataJson) map[string]uint64 {
	paths := make(map[string]uint64)
	for _, img := range meta.Images {
		paths[img.Image.Path] = img.Image.CompressedSize
		if img.Verity != nil {
			paths[img.Verity.Image.Path] = img.Verity.Image.CompressedSize
		}
	}
	if meta.Disk != nil {
		for _, region := range meta.Disk.GptRegions {
			paths[region.Image.Path] = region.Image.CompressedSize
		}
	}
	return paths
}

// partTypeName returns a human-friendly name for a GPT partition type UUID.
func partTypeName(pt metadata.PartitionType) string {
	names := map[metadata.PartitionType]string{
		metadata.PartitionTypeEsp:                "EFI System Partition",
		metadata.PartitionTypeXbootldr:           "Extended Boot Loader",
		metadata.PartitionTypeSwap:               "Swap",
		metadata.PartitionTypeHome:               "Home",
		metadata.PartitionTypeSrv:                "Server Data",
		metadata.PartitionTypeVar:                "Variable Data",
		metadata.PartitionTypeTmp:                "Temporary Data",
		metadata.PartitionTypeLinuxGeneric:       "Linux Generic",
		metadata.PartitionTypeRootAmd64:          "Root (x86-64)",
		metadata.PartitionTypeRootAmd64Verity:    "Root Verity (x86-64)",
		metadata.PartitionTypeRootAmd64VeritySig: "Root Verity Sig (x86-64)",
		metadata.PartitionTypeUsrAmd64:           "Usr (x86-64)",
		metadata.PartitionTypeUsrAmd64Verity:     "Usr Verity (x86-64)",
		metadata.PartitionTypeUsrAmd64VeritySig:  "Usr Verity Sig (x86-64)",
		metadata.PartitionTypeRootArm64:          "Root (AArch64)",
		metadata.PartitionTypeRootArm64Verity:    "Root Verity (AArch64)",
		metadata.PartitionTypeRootArm64VeritySig: "Root Verity Sig (AArch64)",
		metadata.PartitionTypeUsrArm64:           "Usr (AArch64)",
		metadata.PartitionTypeUsrArm64Verity:     "Usr Verity (AArch64)",
		metadata.PartitionTypeUsrArm64VeritySig:  "Usr Verity Sig (AArch64)",
	}
	if n, ok := names[pt]; ok {
		return n
	}
	return "Unknown"
}

// tarTypeDesc returns a human-readable description for a tar typeflag byte.
func tarTypeDesc(flag byte) string {
	switch flag {
	case tar.TypeReg:
		return "file"
	case tar.TypeDir:
		return "dir"
	case tar.TypeSymlink:
		return "symlink"
	case tar.TypeLink:
		return "hardlink"
	case tar.TypeBlock:
		return "block"
	case tar.TypeChar:
		return "char"
	case tar.TypeFifo:
		return "fifo"
	case tar.TypeXHeader:
		return "pax-ext"
	case tar.TypeXGlobalHeader:
		return "pax-global"
	case tar.TypeGNULongName:
		return "gnu-longname"
	case tar.TypeGNULongLink:
		return "gnu-longlink"
	default:
		return fmt.Sprintf("0x%02x", flag)
	}
}

// roundUp rounds n up to the nearest multiple of block.
func roundUp(n, block int64) int64 {
	if n <= 0 {
		return 0
	}
	return ((n + block - 1) / block) * block
}
