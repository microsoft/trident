package virtdeploy

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"path"
)

func buildCloudInitIso(ciConfig *CloudInitConfig, isoPath string) error {
	if ciConfig == nil {
		return fmt.Errorf("cloud init config is nil")
	}

	tempDir, err := os.MkdirTemp("", "my-temp-dir-")
	if err != nil {
		return fmt.Errorf("failed to create temp dir: %w", err)
	}
	defer os.RemoveAll(tempDir)

	err = ciConfig.writeToDir(tempDir)
	if err != nil {
		return fmt.Errorf("failed to write cloud init config to dir: %w", err)
	}

	err = exec.Command(
		"xorrisofs",
		"-o",
		isoPath,
		"-J",
		"-input-charset",
		"utf8",
		"-rational-rock",
		"-V",
		"CIDATA",
		tempDir,
	).Run()
	if err != nil {
		return fmt.Errorf("failed to create cloud init ISO: %w", err)
	}

	return nil
}

func (ci *CloudInitConfig) writeToDir(dir string) error {
	stat, err := os.Stat(dir)
	if err != nil {
		return fmt.Errorf("failed to stat dir '%s': %w", dir, err)
	}

	if !stat.IsDir() {
		return fmt.Errorf("path '%s' is not a directory", dir)
	}

	copyFile := func(src, dst string) error {
		srcFile, err := os.Open(src)
		if err != nil {
			return fmt.Errorf("failed to open source '%s': %w", src, err)
		}
		defer srcFile.Close()

		dstFile, err := os.OpenFile(dst, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0600)
		if err != nil {
			return fmt.Errorf("failed to open destination '%s': %w", dst, err)
		}

		_, copyErr := io.Copy(dstFile, srcFile)
		if copyErr != nil {
			return fmt.Errorf("failed to copy from '%s' to '%s': %w", src, dst, copyErr)
		}

		return nil
	}

	emptyFile := func(filePath string) error {
		f, err := os.OpenFile(filePath, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0600)
		if err != nil {
			return fmt.Errorf("failed to create empty file '%s': %w", filePath, err)
		}
		f.Close()
		return nil
	}

	metadataFile := path.Join(dir, "meta-data")
	if ci.Metadata != "" {
		err = copyFile(ci.Metadata, metadataFile)
		if err != nil {
			return fmt.Errorf("failed to copy metadata file: %w", err)
		}
	} else {
		// If no metadata file is provided, create an empty one to avoid cloud-init errors
		emptyFile(metadataFile)
	}

	userdataFile := path.Join(dir, "user-data")
	if ci.Userdata != "" {
		err = copyFile(ci.Userdata, userdataFile)
		if err != nil {
			return fmt.Errorf("failed to copy userdata file: %w", err)
		}
	} else {
		// If no userdata file is provided, create an empty one to avoid cloud-init errors
		emptyFile(userdataFile)
	}

	return nil

}
