package builder

import (
	"crypto/sha512"
	"fmt"
	"io"
	"os"

	"github.com/klauspost/compress/zstd"
)

const CosiFileExtension = ".cosi"

// DecompressImage decompresses a zstd-compressed image file to a temporary file.
// The caller is responsible for closing and removing the returned file.
func DecompressImage(source string) (*os.File, error) {
	src, err := os.Open(source)
	if err != nil {
		return nil, fmt.Errorf("failed to open %s: %w", source, err)
	}
	defer src.Close()

	tmpFile, err := os.CreateTemp("", "mkcosi")
	if err != nil {
		return nil, fmt.Errorf("failed to create temporary file: %w", err)
	}

	// Configure decoder to support larger window sizes (up to 1GB for --long=30)
	zr, err := zstd.NewReader(src, zstd.WithDecoderMaxWindow(1<<30))
	if err != nil {
		tmpFile.Close()
		return nil, fmt.Errorf("failed to create zstd reader: %w", err)
	}

	if _, err := io.Copy(tmpFile, zr); err != nil {
		tmpFile.Close()
		return nil, fmt.Errorf("failed to decompress %s: %w", source, err)
	}

	zr.Close()

	return tmpFile, nil
}

// Sha384SumReader computes the SHA-384 hash of the data from the reader.
func Sha384SumReader(reader io.Reader) (string, error) {
	sha384 := sha512.New384()
	if _, err := io.Copy(sha384, reader); err != nil {
		return "", err
	}
	return fmt.Sprintf("%x", sha384.Sum(nil)), nil
}
