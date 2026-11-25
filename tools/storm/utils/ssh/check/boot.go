package check

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	stormssh "github.com/microsoft/storm/utils/ssh"
	stormretry "tridenttools/storm/utils/retry"
	stormsshclient "tridenttools/storm/utils/ssh/client"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
)

// WaitForBoot returns an SSH-ready boot distinct from previousBootID; the caller owns the client.
func WaitForBoot(ctx context.Context, settings stormsshconfig.SshCliSettings, previousBootID string) (*stormssh.Client, string, error) {
	// boot pairs a ready connection with the boot identity observed through it.
	type boot struct {
		client *stormssh.Client
		id     string
	}
	result, err := stormretry.RetryContext(ctx, 2*time.Second, func(ctx context.Context, _ int) (*boot, error) {
		attempt, cancel := context.WithTimeout(ctx, 10*time.Second)
		defer cancel()
		client, err := stormsshclient.OpenSshClientContext(attempt, settings)
		if err != nil {
			return nil, err
		}
		out, err := stormsshclient.RunCommandContext(attempt, client, "cat /proc/sys/kernel/random/boot_id")
		if err == nil {
			err = out.Check()
		}
		if err != nil {
			_ = client.Close()
			return nil, err
		}
		id := strings.TrimSpace(out.Stdout)
		parsed, err := uuid.Parse(id)
		if err != nil || parsed == uuid.Nil || id == previousBootID {
			_ = client.Close()
			return nil, fmt.Errorf("boot %q is invalid or unchanged from %q", id, previousBootID)
		}
		return &boot{client: client, id: id}, nil
	})
	if err != nil {
		return nil, "", fmt.Errorf("waiting for SSH boot after %q: %w", previousBootID, err)
	}
	return result.client, result.id, nil
}
