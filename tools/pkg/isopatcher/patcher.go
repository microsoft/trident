/*
Copyright © 2023 Microsoft Corporation
*/
package isopatcher

import (
	"bytes"
	"errors"
	"strings"
)

// PatchFile patches a file within an ISO by replacing a magic string placeholder
// with the provided contents. The placeholder must exist in the ISO and be large
// enough to accommodate the contents.
func PatchFile(iso []byte, filename string, contents []byte) error {
	// Search for magic string
	magicPattern := MagicString + ":" + filename + ":"
	i := bytes.Index(iso, []byte(magicPattern))
	if i == -1 {
		return errors.New("could not find magic string for file: " + filename)
	}

	// Determine placeholder size
	placeholderLength := bytes.IndexByte(iso[i:], byte('\n')) + 1
	if len(contents) > placeholderLength {
		return errors.New("file is too big to fit in placeholder")
	}

	// If the contents are smaller than the placeholder then add a trailing newline.
	if len(contents) < placeholderLength {
		contents = append(contents, byte('\n'))
	}

	// If the contents are still smaller, then pad with # symbols and one final newline.
	if len(contents) < placeholderLength {
		padding := placeholderLength - len(contents)
		contents = append(contents, []byte(strings.Repeat("#", padding-1)+"\n")...)
	}

	copy(iso[i:], contents)

	return nil
}
