package sftp

import (
	"context"
	"errors"
	"fmt"
	"os"

	stormssh "github.com/microsoft/storm/utils/ssh"
)

func DownloadRemoteFile(client *stormssh.Client, remotePath, localPath string) (string, error) {
	files, err := client.SudoSFTP()
	if err != nil {
		return "", fmt.Errorf("open sudo SFTP: %w", err)
	}
	var file *os.File
	if localPath == "" {
		file, err = os.CreateTemp("", "sftp-*")
	} else {
		file, err = os.Create(localPath)
	}
	if err != nil {
		return "", fmt.Errorf("create download destination: %w", err)
	}
	err = errors.Join(files.Download(context.Background(), remotePath, file), file.Close())
	if err != nil {
		if removeErr := os.Remove(file.Name()); removeErr != nil {
			err = errors.Join(err, removeErr)
		}
		return "", fmt.Errorf("download %s: %w", remotePath, err)
	}
	return file.Name(), nil
}
