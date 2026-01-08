// Package provides file utility functions.
package file

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
)

func FindFile(dir, pattern string) (string, error) {
	// Find image file
	regexPattern, e := regexp.Compile(pattern)
	if e != nil {
		return "", fmt.Errorf("failed to match pattern: %w", e)
	}

	matchingFiles := make([]string, 0)
	e = filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() && regexPattern.MatchString(info.Name()) {
			matchingFiles = append(matchingFiles, path)
		}
		return nil
	})
	if e != nil {
		return "", fmt.Errorf("failed to find file: %w", e)
	}
	if len(matchingFiles) < 1 {
		return "", fmt.Errorf("file not found")
	} else if len(matchingFiles) > 1 {
		return "", fmt.Errorf("multiple files found: %v", matchingFiles)
	}
	return matchingFiles[0], nil
}
