// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Utility to discover and validate installation images options

package imageutils

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	CosiExtension = ".cosi"
)

// Scans a directory for available images to install
func DiscoverSystemImages(directoryPath string) ([]SystemImage, error) {
	if directoryPath == "" {
		return nil, fmt.Errorf("directory path cannot be empty")
	}

	info, err := os.Stat(directoryPath)
	if err != nil {
		return nil, fmt.Errorf("failed to access directory '%s': %w", directoryPath, err)
	}

	if !info.IsDir() {
		return nil, fmt.Errorf("path '%s' is not a directory", directoryPath)
	}

	files, err := os.ReadDir(directoryPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read directory '%s': %w", directoryPath, err)
	}

	var images []SystemImage
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
				URL:  "file://" + fullPath,
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
	if _, err := os.Stat(imageFile); os.IsNotExist(err) {
		return false
	}

	if !strings.HasSuffix(imageFile, CosiExtension) {
		return false
	}

	return true
}
