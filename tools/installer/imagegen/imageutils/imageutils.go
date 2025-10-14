// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Utility to discover and validate installation images options

package imageutils

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"installer/internal/file"
)

const (
	CosiExtension = ".cosi"
	FileURLPrefix = "file://"
)

// Scans a directory for available images to install
func DiscoverSystemImages(directoryPath string) (images []SystemImage, err error) {
	if directoryPath == "" {
		return nil, fmt.Errorf("directory path cannot be empty")
	}
	// Convert to absolute path
	directoryPath, err = filepath.Abs(directoryPath)
	if err != nil {
		return nil, fmt.Errorf("failed to get absolute path for directory: %w", err)
	}

	exists, err := file.PathExists(directoryPath)
	if err != nil {
		return
	}
	if !exists {
		return nil, fmt.Errorf("directory does not exist: '%s'", directoryPath)
	}

	isDir, err := file.IsDir(directoryPath)
	if err != nil {
		return
	}
	if !isDir {
		return nil, fmt.Errorf("path is not a directory: '%s'", directoryPath)
	}

	files, err := os.ReadDir(directoryPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read directory '%s': %w", directoryPath, err)
	}

	for _, file := range files {
		// Skip subdirectories
		if file.IsDir() {
			continue
		}

		fileName := file.Name()
		fullPath := filepath.Join(directoryPath, fileName)
		displayName := strings.TrimSuffix(fileName, filepath.Ext(fileName))

		if ValidateImage(fullPath) {
			systemImage := SystemImage{
				Name: displayName,
				URL:  FileURLPrefix + fullPath,
			}
			images = append(images, systemImage)
		}
	}

	if len(images) == 0 {
		return nil, fmt.Errorf("no valid COSI image files found in directory: %s", directoryPath)
	}

	return images, nil
}

// Validate SystemImage
// Current validation just checks if image is a COSI file
func ValidateImage(imageFile string) bool {
	exists, err := file.PathExists(imageFile)
	if err != nil || !exists {
		return false
	}

	if !strings.HasSuffix(imageFile, CosiExtension) {
		return false
	}

	return true
}
