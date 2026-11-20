package sftp

import (
	"context"
	"errors"
	"fmt"
	"os"

	stormssh "github.com/microsoft/storm/facilities/ssh"
)

func DownloadRemoteFile(client *stormssh.Client, remotePath, localPath string) (string, error) {
	var file *os.File
	var err error
	if localPath == "" {
		file, err = os.CreateTemp("", "sftp-*")
	} else {
		file, err = os.Create(localPath)
	}
	if err != nil {
		return "", fmt.Errorf("create download destination: %w", err)
	}
	err = errors.Join(client.Fetch(context.Background(), remotePath, file), file.Close())
	if err != nil {
		if removeErr := os.Remove(file.Name()); removeErr != nil {
			err = errors.Join(err, removeErr)
		}
		return "", fmt.Errorf("download %s: %w", remotePath, err)
	}
	return file.Name(), nil
}
