package virtdeploy

import (
	"fmt"
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

	err = os.WriteFile(path.Join(dir, "user-data"), []byte(ci.Userdata), 0600)
	if err != nil {
		return fmt.Errorf("failed to write user-data: %w", err)
	}

	err = os.WriteFile(path.Join(dir, "meta-data"), []byte(ci.Metadata), 0600)
	if err != nil {
		return fmt.Errorf("failed to write meta-data: %w", err)
	}

	return nil

}
