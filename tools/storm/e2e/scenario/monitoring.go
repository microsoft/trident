package scenario

import (
	"bufio"
	"bytes"
	"container/ring"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"syscall"
	"time"

	stormutils "github.com/microsoft/storm/pkg/storm/utils"
	"github.com/sirupsen/logrus"

	"tridenttools/pkg/ref"
)

const (
	serialPollInterval    = 100 * time.Millisecond
	serialLoginGrace      = time.Minute
	serialShutdownTimeout = 5 * time.Second
	serialProcessWait     = 2 * time.Second
	serialStartupTimeout  = 10 * time.Second
	serialLogTailLines    = 30
)

// Each monitor starts at the current log end, so earlier boots cannot satisfy it.
func (s *TridentE2EScenario) spawnVMSerialMonitor(ctx context.Context, output io.WriteCloser) (<-chan struct{}, error) {
	doneChannel := make(chan struct{})
	if s.hardware != HardwareTypeVM {
		close(doneChannel)
		output.Close()
		return doneChannel, nil
	}

	vmInfo := s.testHost.VmInfo()
	if ref.IsNilInterface(vmInfo) {
		close(doneChannel)
		output.Close()
		return doneChannel, fmt.Errorf("vm host info not set")
	}
	logPath, err := vmInfo.SerialLogPath()
	if err != nil {
		output.Close()
		return nil, err
	}
	monitorCtx, cancel := context.WithCancel(ctx)
	cmd, err := serialLogCommand(monitorCtx, logPath)
	if err != nil {
		cancel()
		output.Close()
		return nil, err
	}
	started := make(chan error, 1)

	go func() {
		defer close(doneChannel)
		defer cancel()
		defer output.Close()

		err := runSerialMonitor(monitorCtx, cancel, cmd, output, started)
		if err != nil {
			errStr := fmt.Sprintf("VM serial log monitor ended with error: %v", err)
			logrus.Error(errStr)

			_, writeErr := output.Write([]byte(fmt.Sprintf("ERROR: %s", errStr)))
			if writeErr != nil {
				logrus.Errorf("failed to write error to VM serial log output: %v", writeErr)
			}
		} else {
			logrus.Infof("VM serial log monitor ended successfully")
		}
	}()

	timer := time.NewTimer(serialStartupTimeout)
	defer timer.Stop()
	select {
	case err := <-started:
		if err == nil {
			return doneChannel, nil
		}
		return nil, errors.Join(err, stopSerialMonitor(cancel, doneChannel))
	case <-ctx.Done():
		return nil, errors.Join(ctx.Err(), stopSerialMonitor(cancel, doneChannel))
	case <-timer.C:
		return nil, errors.Join(
			fmt.Errorf("serial monitor did not start within %s", serialStartupTimeout),
			stopSerialMonitor(cancel, doneChannel))
	}
}

func waitForSerialMonitor(ctx context.Context, cancel context.CancelFunc, done <-chan struct{}) error {
	timer := time.NewTimer(serialLoginGrace)
	defer timer.Stop()
	select {
	case <-done:
		return nil
	case <-ctx.Done():
	case <-timer.C:
		logrus.Info("Waited 1 minute for serial monitor to reach login prompt, cancelling monitor.")
	}
	return stopSerialMonitor(cancel, done)
}

func stopSerialMonitor(cancel context.CancelFunc, done <-chan struct{}) error {
	cancel()
	timer := time.NewTimer(serialShutdownTimeout)
	defer timer.Stop()
	select {
	case <-done:
		return nil
	case <-timer.C:
		return fmt.Errorf("serial monitor did not stop within %s", serialShutdownTimeout)
	}
}

func serialLogCommand(ctx context.Context, path string) (*exec.Cmd, error) {
	info, err := os.Lstat(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, fmt.Errorf("stat serial log %q: %w", path, err)
	}
	if info != nil {
		if !info.Mode().IsRegular() {
			return nil, fmt.Errorf("serial log %q is not a regular file", path)
		}
	} else {
		file, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
		if err != nil {
			return nil, fmt.Errorf("creating serial log %q: %w", path, err)
		}
		if err := file.Close(); err != nil {
			return nil, fmt.Errorf("closing serial log %q: %w", path, err)
		}
	}
	// --pid stops privileged tail even if sudo execs it and we cannot signal it.
	guard := exec.CommandContext(ctx, "sleep", "infinity")
	if err := guard.Start(); err != nil {
		return nil, fmt.Errorf("starting serial log lifetime guard: %w", err)
	}
	go func() {
		if err := guard.Wait(); err != nil && ctx.Err() == nil {
			logrus.Errorf("Serial log lifetime guard failed: %v", err)
		}
	}()
	// Libvirt creates these logs as root, mode 0600. Tail also handles QEMU's
	// truncation on a cold boot without reopening a console stream.
	cmd := exec.CommandContext(ctx, "sudo", "-n", "--", "tail",
		"--lines=0", "--verbose",
		"--follow=name", "--retry",
		"--sleep-interval="+strconv.FormatFloat(serialPollInterval.Seconds(), 'f', -1, 64),
		"--pid="+strconv.Itoa(guard.Process.Pid), "--", path)
	cmd.Cancel = func() error { return nil }
	return cmd, nil
}

func runSerialMonitor(ctx context.Context, cancel context.CancelFunc, cmd *exec.Cmd, out io.Writer, started chan<- error) error {
	reader, writer := io.Pipe()
	defer reader.Close()
	var stderr bytes.Buffer
	cmd.Stdout = writer
	cmd.Stderr = &stderr
	cmd.WaitDelay = serialProcessWait
	if err := cmd.Start(); err != nil {
		writer.Close()
		err = fmt.Errorf("starting serial log monitor: %w", err)
		started <- err
		return err
	}
	done := make(chan error, 1)
	go func() {
		err := cmd.Wait()
		if err != nil {
			err = fmt.Errorf("following serial log: %w: %s", err, stderr.String())
		}
		writer.CloseWithError(err)
		done <- err
	}()
	stop := context.AfterFunc(ctx, func() { _ = reader.CloseWithError(ctx.Err()) })
	defer stop()
	buffered := bufio.NewReader(reader)
	err := serialFollowerReady(buffered)
	started <- err
	if err == nil {
		err = readerLoop(ctx, buffered, out, serialLogTailLines)
	}
	cause := ctx.Err()
	cancel()
	reader.Close()
	commandErr := <-done
	if commandErr != nil && !errors.Is(commandErr, context.Canceled) {
		var exitErr *exec.ExitError
		expectedTermination := false
		if errors.As(commandErr, &exitErr) {
			status, ok := exitErr.Sys().(syscall.WaitStatus)
			expectedTermination = ok && status.Signal() == syscall.SIGTERM
		}
		if !expectedTermination {
			err = errors.Join(err, commandErr)
		}
	}
	return errors.Join(err, cause)
}

func serialFollowerReady(reader *bufio.Reader) error {
	// Tail flushes its verbose header after opening and positioning the log.
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			return fmt.Errorf("starting serial log follower: %w", err)
		}
		if line == "\n" {
			continue
		}
		if strings.HasPrefix(line, "==> ") && strings.HasSuffix(line, " <==\n") {
			return nil
		}
		return fmt.Errorf("unexpected serial follower startup output: %q", line)
	}
}

func readerLoop(ctx context.Context, in io.Reader, out io.Writer, ringSize int) error {
	ring := ring.New(ringSize)

	reader := bufio.NewReader(in)
	var lineBuffBuilder strings.Builder
	for {
		if ctx.Err() != nil {
			logrus.Errorf("VM serial monitor was cancelled. Last %d lines of VM serial log before timeout:\n%s", ringSize, func() string {
				var sb strings.Builder
				ring.Do(func(p interface{}) {
					if p != nil {
						sb.WriteString(p.(string) + "\n")
					}
				})
				return sb.String()
			}())

			return ctx.Err()
		}

		// The installer ("mos") also exposes a login prompt.
		if strings.Contains(lineBuffBuilder.String(), "login:") &&
			!strings.Contains(lineBuffBuilder.String(), "mos") {
			logrus.Infof("Login prompt found in VM serial log")
			return nil
		}

		readRune, _, err := reader.ReadRune()
		if errors.Is(err, io.EOF) {
			return fmt.Errorf("serial log follower ended before the login prompt")
		} else if err != nil {
			return fmt.Errorf("failed to read from serial log file: %w", err)
		}

		if readRune == '\n' {
			ring.Value = lineBuffBuilder.String()
			ring = ring.Next()

			_, err := out.Write([]byte(stormutils.RemoveAllANSI(lineBuffBuilder.String()) + "\n"))
			if err != nil {
				return fmt.Errorf("failed to write serial log output: %w", err)
			}

			lineBuffBuilder.Reset()
		} else {
			lineBuffBuilder.WriteRune(readRune)
		}
	}
}
