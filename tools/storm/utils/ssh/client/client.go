package client

import (
	"context"
	"fmt"

	stormssh "github.com/microsoft/storm/utils/ssh"

	"tridenttools/storm/utils/ssh/config"
	"tridenttools/storm/utils/ssh/sftp"
	"tridenttools/storm/utils/sshutils"
)

func OpenSshClient(settings config.SshCliSettings) (*stormssh.Client, error) {
	return OpenSshClientContext(context.Background(), settings)
}

func OpenSshClientContext(ctx context.Context, settings config.SshCliSettings) (*stormssh.Client, error) {
	cfg, err := settings.IntoClientConfig()
	if err != nil {
		return nil, err
	}
	return sshutils.CreateSshClient(ctx, *cfg)
}

func RunCommand(client sshutils.CommandRunner, command string) (*config.SshCmdOutput, error) {
	return RunCommandContext(context.Background(), client, command)
}

func RunCommandContext(ctx context.Context, client sshutils.CommandRunner, command string) (*config.SshCmdOutput, error) {
	return sshutils.RunCommandContext(ctx, client, command)
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
