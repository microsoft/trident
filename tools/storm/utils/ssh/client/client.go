package client

import (
	"context"
	"fmt"

	stormssh "github.com/microsoft/storm/facilities/ssh"

	"tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/ssh/sftp"
	"tridenttools/storm/utils/sshutils"
)

func OpenSshClient(settings config.SshCliSettings) (*stormssh.Client, error) {
	cfg, err := settings.IntoClientConfig()
	if err != nil {
		return nil, err
	}
	return sshutils.CreateSshClient(context.Background(), *cfg)
}

func RunCommand(client *stormssh.Client, command string) (*config.SshCmdOutput, error) {
	return sshutils.RunCommand(client, command)
}

func CommandOutput(client *stormssh.Client, command string) (string, error) {
	return sshutils.CommandOutput(client, command)
}

func CopyRemoteFileToLocal(client *stormssh.Client, remotePath, localFilePath string) error {
	if _, err := sftp.DownloadRemoteFile(client, remotePath, localFilePath); err != nil {
		return fmt.Errorf("copy remote file %s: %w", remotePath, err)
	}
	return nil
}
