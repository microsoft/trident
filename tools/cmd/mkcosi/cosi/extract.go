package cosi

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"

	log "github.com/sirupsen/logrus"
)

func extractCosiFile(reader io.ReadSeeker) (string, []string, error) {
	tmpDir, err := os.MkdirTemp("", "cosi-extract-*")
	if err != nil {
		return "", nil, fmt.Errorf("failed to create temporary directory: %w", err)
	}

	_, err = reader.Seek(0, io.SeekStart)
	if err != nil {
		return "", nil, fmt.Errorf("failed to seek to start of COSI file: %w", err)
	}

	tarCmd := exec.Command("tar", "-xvf", "-", "-C", tmpDir)
	tarCmd.Stdin = reader
	out, err := tarCmd.CombinedOutput()
	if err != nil {
		log.Errorf("Failed to extract COSI file:\n%v", string(out))
		return "", nil, fmt.Errorf("failed to extract COSI file: %w", err)
	}

	contents := make([]string, 0)
	err = filepath.Walk(tmpDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return fmt.Errorf("failed to walk directory: %w", err)
		}

		if !info.IsDir() {
			relPath, err := filepath.Rel(tmpDir, path)
			if err != nil {
				return fmt.Errorf("failed to get relative path: %w", err)
			}

			contents = append(contents, relPath)
			log.WithField("path", relPath).Debug("Found file in COSI file")
		}

		return nil
	})
	if err != nil {
		return "", nil, fmt.Errorf("failed to walk directory: %w", err)
	}

	return tmpDir, contents, nil
}
