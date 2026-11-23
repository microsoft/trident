package virtdeploy

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/microsoft/storm/utils/iso"
)

func buildCloudInitIso(ciConfig *CloudInitConfig, isoPath string) error {
	if ciConfig == nil {
		return fmt.Errorf("cloud init config is nil")
	}
	dir, err := os.MkdirTemp("", "trident-cloud-init-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(dir)
	for name, source := range map[string]string{
		"user-data": ciConfig.Userdata,
		"meta-data": ciConfig.Metadata,
	} {
		var data []byte
		if source != "" {
			var err error
			data, err = os.ReadFile(source)
			if err != nil {
				return fmt.Errorf("read cloud-init %s: %w", name, err)
			}
		}
		if err := os.WriteFile(filepath.Join(dir, name), data, 0600); err != nil {
			return fmt.Errorf("write cloud-init %s: %w", name, err)
		}
	}
	return iso.Build(context.Background(), isoPath, "CIDATA", os.DirFS(dir))
}
