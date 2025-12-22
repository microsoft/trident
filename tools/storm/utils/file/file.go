// Package provides file utility functions.
package file

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"time"
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

func WaitForFileToExist(ctx context.Context, filePath string) error {
	for {
		if _, err := os.Stat(filePath); err == nil {
			return nil
		}

		if ctx.Err() != nil {
			return ctx.Err()
		}

		// Sleep for a short duration before checking again
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
}

func FileExists(filePath string) (bool, error) {
	_, err := os.Stat(filePath)
	if err == nil {
		return true, nil
	}
	if errors.Is(err, fs.ErrNotExist) {
		return false, nil
	}
	return false, err
}
