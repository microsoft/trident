// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package userutils

import (
	"fmt"
	"path/filepath"
	"strconv"
	"strings"

	"tridenttools/azltools/internal/file"
)

type PasswdEntry struct {
	Name          string
	Uid           int
	Gid           int
	Description   string
	HomeDirectory string
	Shell         string
}

func ReadPasswdFile(rootDir string) ([]PasswdEntry, error) {
	lines, err := file.ReadLines(filepath.Join(rootDir, PasswdFile))
	if err != nil {
		return nil, fmt.Errorf("failed to read %s file:\n%w", PasswdFile, err)
	}

	entries, err := parsePasswdFile(lines)
	if err != nil {
		return nil, fmt.Errorf("invalid %s file:\n%w", PasswdFile, err)
	}

	return entries, nil
}

func parsePasswdFile(lines []string) ([]PasswdEntry, error) {
	entries := []PasswdEntry(nil)
	for i, line := range lines {
		entry, err := parsePasswdFileEntry(line)
		if err != nil {
			return nil, fmt.Errorf("invalid line %d:\n%w", i, err)
		}

		entries = append(entries, entry)
	}

	return entries, nil
}

func parsePasswdFileEntry(line string) (PasswdEntry, error) {
	const (
		numFields = 7
	)

	fields := strings.Split(line, ":")
	if len(fields) != numFields {
		return PasswdEntry{}, fmt.Errorf("%d fields instead of %d", len(fields), numFields)
	}

	uidStr := fields[2]
	uid, err := strconv.Atoi(uidStr)
	if err != nil {
		return PasswdEntry{}, fmt.Errorf("invalid UID (%s):\n%w", uidStr, err)
	}

	gidStr := fields[3]
	gid, err := strconv.Atoi(gidStr)
	if err != nil {
		return PasswdEntry{}, fmt.Errorf("invalid GID (%s):\n%w", gidStr, err)
	}

	entry := PasswdEntry{
		Name:          fields[0],
		Uid:           uid,
		Gid:           gid,
		Description:   fields[4],
		HomeDirectory: fields[5],
		Shell:         fields[6],
	}
	return entry, nil
}
