// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Parser for the image builder's configuration schemas.

package configuration

import (
	"encoding/json"
	"fmt"
	"sort"
	"strconv"

	"tridenttools/azltools/internal/logger"
)

// TargetDisk [kickstart-only] defines the physical disk, to which
// Azure Linux should be installed.
type TargetDisk struct {
	Type  string `json:"Type"`
	Value string `json:"Value"`
}

// RootEncryption enables encryption on the root partition
type RootEncryption struct {
	Enable   bool   `json:"Enable"`
	Password string `json:"Password"`
}

// Disk holds the disk partitioning, formatting and size information.
type Disk struct {
	PartitionTableType PartitionTableType `json:"PartitionTableType"`
	MaxSize            uint64             `json:"MaxSize"`
	TargetDisk         TargetDisk         `json:"TargetDisk"`
	Partitions         []Partition        `json:"Partitions"`
}

// checkOverlappingPartitions checks that start and end positions of the defined partitions don't overlap.
func checkOverlappingePartitions(disk *Disk) (err error) {
	partIntervals := [][]uint64{}
	//convert  partition entries to array of [start,end] locations
	for _, part := range disk.Partitions {
		partIntervals = append(partIntervals, []uint64{part.Start, part.End})
	}
	//sorting paritions by start position
	sort.Slice(partIntervals, func(i, j int) bool {
		return partIntervals[i][0] < partIntervals[j][0]
	})
	//confirm each partition ends before the next starts
	for i := 0; i < len(partIntervals)-1; i++ {
		if partIntervals[i][1] > partIntervals[i+1][0] {
			return fmt.Errorf("a [Partition] with an end location %d overlaps with a [Partition] with a start location %d", partIntervals[i][1], partIntervals[i+1][0])
		}
	}
	return
}

// checkMaxSizeCorrectness checks that MaxSize is non-zero for cases in which it's used to clear disk space. This check
// also confirms that the MaxSize defined is large enough to accomodate all partitions. No partition should have an
// end position that exceeds the MaxSize
func checkMaxSizeCorrectness(disk *Disk) (err error) {
	const (
		realDiskType = "path"
	)
	//MaxSize is not relevant if target disk is specified.
	if disk.TargetDisk.Type != realDiskType {
		//Complain about 0 maxSize only when partitions are defined.
		if disk.MaxSize <= 0 && len(disk.Partitions) != 0 {
			return fmt.Errorf("a configuration without a defined target disk must have a non-zero MaxSize")
		}
		lastPartitionEnd := uint64(0)
		maxSize := disk.MaxSize
		//check last parition end location does not surpass MaxSize
		for _, part := range disk.Partitions {
			if part.End == 0 {
				lastPartitionEnd = part.Start
			} else if part.End > lastPartitionEnd {
				lastPartitionEnd = part.End
			}
		}
		maxSizeString := strconv.FormatUint(maxSize, 10)
		lastPartitionEndString := strconv.FormatUint(lastPartitionEnd, 10)
		if maxSize < lastPartitionEnd {
			return fmt.Errorf("the MaxSize of %s is not large enough to accomodate defined partitions ending at %s", maxSizeString, lastPartitionEndString)
		}
	} else if disk.MaxSize != 0 {
		logger.Log.Warnf("defining both a maxsize and target disk in the same config should be avoided as maxsize value will not be used")
	}
	return
}

// IsValid returns an error if the PartitionTableType is not valid
func (d *Disk) IsValid() (err error) {
	if err = d.PartitionTableType.IsValid(); err != nil {
		return fmt.Errorf("invalid [PartitionTableType]: %w", err)
	}

	err = checkOverlappingePartitions(d)
	if err != nil {
		return fmt.Errorf("invalid [Disk]: %w", err)
	}

	err = checkMaxSizeCorrectness(d)
	if err != nil {
		return fmt.Errorf("invalid [Disk]: %w", err)
	}

	for _, partition := range d.Partitions {
		if err = partition.IsValid(); err != nil {
			return
		}
	}
	return
}

// UnmarshalJSON Unmarshals a Disk entry
func (d *Disk) UnmarshalJSON(b []byte) (err error) {
	// Use an intermediate type which will use the default JSON unmarshal implementation
	type IntermediateTypeDisk Disk
	err = json.Unmarshal(b, (*IntermediateTypeDisk)(d))
	if err != nil {
		return fmt.Errorf("failed to parse [Disk]: %w", err)
	}

	// Now validate the resulting unmarshalled object
	err = d.IsValid()
	if err != nil {
		return fmt.Errorf("failed to parse [Disk]: %w", err)
	}
	return
}

// GetDiskPartByID returns the disk partition object with the desired ID, nil if no partition found
func (c *Config) GetDiskPartByID(ID string) (diskPart *Partition) {
	for i, d := range c.Disks {
		for j, p := range d.Partitions {
			if p.ID == ID {
				return &c.Disks[i].Partitions[j]
			}
		}
	}
	return nil
}

// GetDiskContainingPartition returns the disk containing the provided partition
func (c *Config) GetDiskContainingPartition(partition *Partition) (disk *Disk) {
	ID := partition.ID
	for i, d := range c.Disks {
		for _, p := range d.Partitions {
			if p.ID == ID {
				return &c.Disks[i]
			}
		}
	}
	return nil
}

func (c *Config) GetBootPartition() (partitionIndex int, partition *Partition) {
	for i, d := range c.Disks {
		for j, p := range d.Partitions {
			if p.HasFlag(PartitionFlagBoot) {
				return j, &c.Disks[i].Partitions[j]
			}
		}
	}
	return
}
