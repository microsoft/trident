package sha384

import (
	"crypto/sha512"
	"encoding/hex"
	"fmt"
	"io"
	"os"
)

func CalculateSha384(filePath string) (string, error) {
	// Hash the .raw file
	file, err := os.Open(filePath)
	if err != nil {
		return "", fmt.Errorf("failed to open %s: %w", filePath, err)
	}
	defer file.Close()
	hasher := sha512.New384()
	if _, err := io.Copy(hasher, file); err != nil {
		return "", fmt.Errorf("failed to calculate hash: %w", err)
	}
	return hex.EncodeToString(hasher.Sum(nil)), nil
}
