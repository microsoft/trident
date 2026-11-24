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

// UploadLocalFile copies a local file through the same privileged SFTP transport as downloads.
type sudoClient interface {
	SudoSFTP(...string) (*stormssh.SFTP, error)
}

func UploadLocalFile(client sudoClient, localPath, remotePath string) error {
	return UploadLocalFileContext(context.Background(), client, localPath, remotePath)
}

func UploadLocalFileContext(ctx context.Context, client sudoClient, localPath, remotePath string) error {
	source, err := os.Open(localPath)
	if err != nil {
		return err
	}
	remote, err := client.SudoSFTP()
	if err != nil {
		return errors.Join(err, source.Close())
	}
	return errors.Join(remote.Upload(ctx, remotePath, source), source.Close())
}
