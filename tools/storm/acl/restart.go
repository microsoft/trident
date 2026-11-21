package acl

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"regexp"
	"strings"
	"time"

	"golang.org/x/crypto/ssh"
	stormsshconfig "tridenttools/storm/utils/ssh/config"
	stormtrident "tridenttools/storm/utils/trident"
)

var guestRestartBoundary = regexp.MustCompile(`(?m)^\[\s*\d+\.\d+\] reboot: Restarting system\r?$`)

// waitForGuestRestart tails serial output until Linux reaches its final restart boundary.
func waitForGuestRestart(ctx context.Context, serial io.Reader) ([]byte, error) {
	var output bytes.Buffer
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()
	for {
		if err := ctx.Err(); err != nil {
			return output.Bytes(), fmt.Errorf("waiting for guest restart boundary: %w", err)
		}
		if _, err := io.Copy(&output, serial); err != nil {
			return output.Bytes(), err
		}
		if guestRestartBoundary.Match(output.Bytes()) {
			return output.Bytes(), nil
		}
		select {
		case <-ctx.Done():
		case <-ticker.C:
		}
	}
}

// invokeTridentWithGuestRestart requires both Trident's reboot announcement and a serial restart boundary.
// It closes SSH when the serial wait finishes so a stalled warm reset cannot hang the command.
func invokeTridentWithGuestRestart(ctx context.Context, client *ssh.Client, arguments string, serial io.Reader) (*stormsshconfig.SshCmdOutput, []byte, error) {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	// restartResult carries the observed shutdown excerpt and any serial-wait error.
	type restartResult struct {
		serial []byte
		err    error
	}
	restarted := make(chan restartResult, 1)
	go func() {
		output, err := waitForGuestRestart(ctx, serial)
		// A stalled warm reset can leave SSH waiting on a connection that will never close.
		_ = client.Close()
		restarted <- restartResult{output, err}
	}()
	out, invokeErr := stormtrident.InvokeTrident(stormtrident.RuntimeTypeHost, client, nil, arguments)
	result := stormtrident.CheckCommandResult(out, invokeErr, true)
	if result == nil && !strings.Contains(out.Stderr, stormtrident.REBOOTING_LOG_MESSAGE) {
		result = errors.New("Trident did not request reboot")
	}
	if result != nil {
		cancel()
	}
	boundary := <-restarted
	return out, boundary.serial, errors.Join(result, boundary.err)
}

// tridentWithRestart saves fresh restart evidence before cold-cycling the existing MSHV domain.
func (r *runner) tridentWithRestart(ctx context.Context, client *ssh.Client, arguments, logName string) (*stormsshconfig.SshCmdOutput, error) {
	serial, err := os.Open(r.qemu.SerialLog)
	if err != nil {
		return nil, err
	}
	defer serial.Close()
	// Ignore restart markers from earlier boots in this console file.
	if _, err := serial.Seek(0, io.SeekEnd); err != nil {
		return nil, err
	}
	restartCtx, cancel := context.WithTimeout(ctx, r.args.BootTimeout)
	defer cancel()
	out, shutdown, err := invokeTridentWithGuestRestart(restartCtx, client, arguments, serial)
	r.closeClient()
	if err := errors.Join(err, r.save(logName+"-restart-serial.log", shutdown)); err != nil {
		return out, err
	}
	if _, err := fmt.Fprintln(r.log, "confirmed guest restart boundary; cold-cycling existing MSHV domain"); err != nil {
		return out, err
	}
	startCtx, done := context.WithTimeout(ctx, r.args.BootTimeout)
	defer done()
	if err := r.qemu.ColdRestartQemuVM(startCtx, r.vmName); err != nil {
		return out, err
	}
	r.ip = ""
	return out, nil
}
