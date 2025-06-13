// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

package imagecustomizerlib

import (
	"fmt"
	"path/filepath"

	"tridenttools/azltools/internal/file"
	"tridenttools/azltools/internal/logger"
	"tridenttools/azltools/internal/safechroot"
)

func UpdateHostname(hostname string, imageChroot safechroot.ChrootInterface) error {
	if hostname == "" {
		return nil
	}

	logger.Log.Infof("Setting hostname (%s)", hostname)

	hostnameFilePath := filepath.Join(imageChroot.RootDir(), "etc/hostname")
	err := file.Write(hostname, hostnameFilePath)
	if err != nil {
		return fmt.Errorf("failed to write hostname file: %w", err)
	}

	return nil
}
