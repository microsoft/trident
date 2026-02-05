// Package gpt provides functionality for parsing and extracting GPT (GUID Partition Table) data.
package gpt

import (
	"encoding/binary"
	"fmt"
	"io"
	"os"

	"github.com/google/uuid"
)

const (
	// LBASize is the default logical block address size in bytes.
	LBASize = 512
	// GPTSignature is the signature present at the start of the GPT header.
	GPTSignature = "EFI PART"
	// GPTHeaderSize is the minimum size of the GPT header in bytes.
	GPTHeaderSize = 92
)

// Header represents the GPT header structure.
type Header struct {
	Signature                [8]byte
	Revision                 uint32
	HeaderSize               uint32
	HeaderCRC32              uint32
	Reserved                 uint32
	CurrentLBA               uint64
	BackupLBA                uint64
	FirstUsableLBA           uint64
	LastUsableLBA            uint64
	DiskGUID                 uuid.UUID
	PartitionEntriesLBA      uint64
	NumberOfPartitionEntries uint32
	SizeOfPartitionEntry     uint32
	PartitionEntriesCRC32    uint32
}

// PartitionEntry represents a single GPT partition entry.
type PartitionEntry struct {
	PartitionTypeGUID   uuid.UUID
	UniquePartitionGUID uuid.UUID
	StartingLBA         uint64
	EndingLBA           uint64
	Attributes          uint64
	PartitionName       [72]byte // UTF-16LE encoded
}

// ParsedGPT contains the parsed GPT data.
type ParsedGPT struct {
	Header     Header
	Partitions []PartitionEntry
	// PrimaryGPTEndOffset is the byte offset where the GPT entries end
	// (i.e., the end of the region that should be extracted for COSI).
	PrimaryGPTEndOffset uint64
	// LBASize is the logical block address size in bytes.
	LBASize uint64
}

// GetName returns the partition name as a string.
func (p *PartitionEntry) GetName() string {
	// Convert UTF-16LE to string
	var name []rune
	for i := 0; i < len(p.PartitionName); i += 2 {
		r := rune(p.PartitionName[i]) | rune(p.PartitionName[i+1])<<8
		if r == 0 {
			break
		}
		name = append(name, r)
	}
	return string(name)
}

// IsEmpty returns true if this is an empty/unused partition entry.
func (p *PartitionEntry) IsEmpty() bool {
	return p.PartitionTypeGUID == uuid.Nil
}

// SizeInBytes returns the size of the partition in bytes.
func (p *PartitionEntry) SizeInBytes(lbaSize uint64) uint64 {
	// EndingLBA is inclusive, so we add 1
	return (p.EndingLBA - p.StartingLBA + 1) * lbaSize
}

// StartOffset returns the starting byte offset of the partition.
func (p *PartitionEntry) StartOffset(lbaSize uint64) uint64 {
	return p.StartingLBA * lbaSize
}

// parseUUID parses a GUID in mixed-endian format as used in GPT.
// The first three components are little-endian, the rest is big-endian.
func parseUUID(data []byte) uuid.UUID {
	if len(data) < 16 {
		return uuid.Nil
	}
	// GPT uses mixed-endian format for GUIDs
	var result uuid.UUID
	// First 4 bytes: little-endian
	result[0] = data[3]
	result[1] = data[2]
	result[2] = data[1]
	result[3] = data[0]
	// Next 2 bytes: little-endian
	result[4] = data[5]
	result[5] = data[4]
	// Next 2 bytes: little-endian
	result[6] = data[7]
	result[7] = data[6]
	// Last 8 bytes: big-endian (straight copy)
	copy(result[8:], data[8:16])
	return result
}

// ParseGPT reads and parses the GPT from the given reader.
// The reader should be positioned at the start of the disk image.
func ParseGPT(reader io.ReaderAt, diskSize uint64) (*ParsedGPT, error) {
	lbaSize := uint64(LBASize)

	// Read the GPT header from LBA 1 (after the protective MBR at LBA 0)
	headerData := make([]byte, LBASize)
	_, err := reader.ReadAt(headerData, int64(lbaSize)) // LBA 1
	if err != nil {
		return nil, fmt.Errorf("failed to read GPT header: %w", err)
	}

	// Verify the GPT signature
	if string(headerData[:8]) != GPTSignature {
		return nil, fmt.Errorf("invalid GPT signature: expected %q, got %q", GPTSignature, string(headerData[:8]))
	}

	// Parse the header
	header := Header{
		Revision:                 binary.LittleEndian.Uint32(headerData[8:12]),
		HeaderSize:               binary.LittleEndian.Uint32(headerData[12:16]),
		HeaderCRC32:              binary.LittleEndian.Uint32(headerData[16:20]),
		Reserved:                 binary.LittleEndian.Uint32(headerData[20:24]),
		CurrentLBA:               binary.LittleEndian.Uint64(headerData[24:32]),
		BackupLBA:                binary.LittleEndian.Uint64(headerData[32:40]),
		FirstUsableLBA:           binary.LittleEndian.Uint64(headerData[40:48]),
		LastUsableLBA:            binary.LittleEndian.Uint64(headerData[48:56]),
		DiskGUID:                 parseUUID(headerData[56:72]),
		PartitionEntriesLBA:      binary.LittleEndian.Uint64(headerData[72:80]),
		NumberOfPartitionEntries: binary.LittleEndian.Uint32(headerData[80:84]),
		SizeOfPartitionEntry:     binary.LittleEndian.Uint32(headerData[84:88]),
		PartitionEntriesCRC32:    binary.LittleEndian.Uint32(headerData[88:92]),
	}
	copy(header.Signature[:], headerData[:8])

	// Validate header size
	if header.HeaderSize < GPTHeaderSize {
		return nil, fmt.Errorf("invalid GPT header size: %d (minimum is %d)", header.HeaderSize, GPTHeaderSize)
	}

	// Calculate the size of all partition entries
	partitionEntriesSize := uint64(header.NumberOfPartitionEntries) * uint64(header.SizeOfPartitionEntry)

	// Calculate where the partition entries end
	partitionEntriesStart := header.PartitionEntriesLBA * lbaSize
	primaryGPTEndOffset := partitionEntriesStart + partitionEntriesSize

	// Read partition entries
	partitionData := make([]byte, partitionEntriesSize)
	_, err = reader.ReadAt(partitionData, int64(partitionEntriesStart))
	if err != nil {
		return nil, fmt.Errorf("failed to read partition entries: %w", err)
	}

	// Parse partition entries
	partitions := make([]PartitionEntry, 0)
	for i := uint32(0); i < header.NumberOfPartitionEntries; i++ {
		offset := i * header.SizeOfPartitionEntry
		entryData := partitionData[offset : offset+header.SizeOfPartitionEntry]

		entry := PartitionEntry{
			PartitionTypeGUID:   parseUUID(entryData[0:16]),
			UniquePartitionGUID: parseUUID(entryData[16:32]),
			StartingLBA:         binary.LittleEndian.Uint64(entryData[32:40]),
			EndingLBA:           binary.LittleEndian.Uint64(entryData[40:48]),
			Attributes:          binary.LittleEndian.Uint64(entryData[48:56]),
		}
		copy(entry.PartitionName[:], entryData[56:128])

		// Only include non-empty partitions
		if !entry.IsEmpty() {
			partitions = append(partitions, entry)
		}
	}

	return &ParsedGPT{
		Header:              header,
		Partitions:          partitions,
		PrimaryGPTEndOffset: primaryGPTEndOffset,
		LBASize:             lbaSize,
	}, nil
}

// ExtractPrimaryGPTRegion reads the primary GPT region (protective MBR + GPT header + entries)
// from the given reader and returns it as a byte slice.
func ExtractPrimaryGPTRegion(reader io.ReaderAt, gpt *ParsedGPT) ([]byte, error) {
	data := make([]byte, gpt.PrimaryGPTEndOffset)
	_, err := reader.ReadAt(data, 0)
	if err != nil {
		return nil, fmt.Errorf("failed to read primary GPT region: %w", err)
	}
	return data, nil
}

// ParseGPTFromFile opens a file and parses the GPT from it.
// If vhdSize is non-zero, it indicates the file is a fixed VHD and the effective
// disk size should be reduced by the VHD footer size.
func ParseGPTFromFile(path string, vhdFooterSize int64) (*ParsedGPT, *os.File, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to open file: %w", err)
	}

	stat, err := file.Stat()
	if err != nil {
		file.Close()
		return nil, nil, fmt.Errorf("failed to stat file: %w", err)
	}

	diskSize := uint64(stat.Size() - vhdFooterSize)

	gpt, err := ParseGPT(file, diskSize)
	if err != nil {
		file.Close()
		return nil, nil, err
	}

	return gpt, file, nil
}
