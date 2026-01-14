package scenario

import (
	"context"
	"tridenttools/storm/utils/sshutils"
)

// waitForSshToDisconnect waits for the SSH client to disconnect, indicating
// that the remote host is rebooting or shutting down. It uses the provided
// context for timeout and cancellation.
//
// If the SSH client is nil, this function is a no-op and returns nil.
//
// If an error occurs while waiting for the disconnect, that error is returned.
// On success, the SSH client is closed and set to nil.
func (s *TridentE2EScenario) waitForSshToDisconnect(ctx context.Context) error {
	if s.sshClient == nil {
		// No SSH client, nothing to do
		return nil
	}

	err := sshutils.WaitForDisconnect(ctx, s.sshClient)
	if err != nil {
		return err
	}

	// On success, close and clear the SSH client.
	s.sshClient.Close()
	s.sshClient = nil

	return nil
}
