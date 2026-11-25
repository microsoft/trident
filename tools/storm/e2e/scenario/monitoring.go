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
	"strings"
	"syscall"
	"time"

	stormutils "github.com/microsoft/storm/pkg/storm/utils"
	"github.com/sirupsen/logrus"
	"golang.org/x/sys/unix"

	"tridenttools/pkg/ref"
)

const (
	serialLoginGrace      = time.Minute
	serialShutdownTimeout = 5 * time.Second
	serialRetryInterval   = 100 * time.Millisecond
	serialLogTailLines    = 30
)

// spawnVMSerialMonitor starts the VM serial monitor for the test host IF it is a
// VM.
//
// The output of the serial monitor will be written live to the provided
// io.WriteCloser. The WriteCloser will be closed when the monitor exits.
//
// If an error occurs while starting the monitor, that error is returned.
//
// The returned channel can be used to wait for the monitor to finish by waiting
// on it for a value. The channel will be closed once the monitor has finished.
// If the monitor doesn't run (because the hardware type is not VM), the channel
// will receive a value immediately.
//
// The monitor will run until the context is cancelled or the login prompt is
// detected.
func (s *TridentE2EScenario) spawnVMSerialMonitor(ctx context.Context, output io.WriteCloser) (<-chan bool, error) {
	// Channel to signal when the monitor is done. Buffered with size 1 to avoid
	// deadlocks when we exit early and send a message to it immediately.
	doneChannel := make(chan bool, 1)

	// Only spawn the VM serial logger if the hardware type is VM. Otherwise, do
	// nothing.
	if s.hardware != HardwareTypeVM {
		// Immediately signal that the monitor is done
		doneChannel <- true
		close(doneChannel)
		output.Close()
		return doneChannel, nil
	}

	// Get VM info
	vmInfo := s.testHost.VmInfo()
	if ref.IsNilInterface(vmInfo) {
		close(doneChannel)
		output.Close()
		return doneChannel, fmt.Errorf("vm host info not set")
	}

	go func() {
		defer func() {
			// On exit signal that we're done and close the channel.
			doneChannel <- true
			close(doneChannel)
		}()
		defer output.Close()
		monitorCtx, cancel := context.WithCancel(ctx)
		defer cancel()

		err := monitorVMConsole(monitorCtx, cancel, vmInfo.VmName(), output)
		if err != nil {
			errStr := fmt.Sprintf("VM serial log monitor ended with error: %v", err)
			logrus.Error(errStr)

			// Best effort write to output
			_, writeErr := output.Write([]byte(fmt.Sprintf("ERROR: %s", errStr)))
			if writeErr != nil {
				logrus.Errorf("failed to write error to VM serial log output: %v", writeErr)
			}
		} else {
			logrus.Infof("VM serial log monitor ended successfully")
		}
	}()

	return doneChannel, nil
}

func waitForSerialMonitor(ctx context.Context, cancel context.CancelFunc, done <-chan bool) error {
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

func stopSerialMonitor(cancel context.CancelFunc, done <-chan bool) error {
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

// virsh owns the stream on a separate root-less connection. Canceling its process
// avoids go-libvirt's response-router deadlock without touching the control client.
func monitorVMConsole(ctx context.Context, cancel context.CancelFunc, name string, out io.Writer) error {
	// virsh requires a controlling terminal even when console output is piped.
	master, err := os.OpenFile("/dev/ptmx", os.O_RDWR|syscall.O_NOCTTY, 0)
	if err != nil {
		return fmt.Errorf("opening console terminal: %w", err)
	}
	defer master.Close()
	if err := unix.IoctlSetPointerInt(int(master.Fd()), unix.TIOCSPTLCK, 0); err != nil {
		return fmt.Errorf("unlocking console terminal: %w", err)
	}
	number, err := unix.IoctlGetInt(int(master.Fd()), unix.TIOCGPTN)
	if err != nil {
		return fmt.Errorf("locating console terminal: %w", err)
	}
	terminal, err := os.OpenFile(fmt.Sprintf("/dev/pts/%d", number), os.O_RDWR|syscall.O_NOCTTY, 0)
	if err != nil {
		return fmt.Errorf("opening console terminal slave: %w", err)
	}
	defer terminal.Close()
	return runSerialMonitor(ctx, cancel, func() *exec.Cmd {
		cmd := exec.CommandContext(ctx, "virsh", "--connect", "qemu:///system", "console", name, "--force")
		cmd.Stdin = terminal
		cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true, Setctty: true}
		return cmd
	}, out)
}

func runSerialMonitor(ctx context.Context, cancel context.CancelFunc, command func() *exec.Cmd, out io.Writer) error {
	reader, writer := io.Pipe()
	defer reader.Close()
	done := make(chan error, 1)
	go func() {
		ticker := time.NewTicker(serialRetryInterval)
		defer ticker.Stop()
		for {
			var stderr bytes.Buffer
			cmd := command()
			cmd.Stdout = writer
			cmd.Stderr = &stderr
			cmd.WaitDelay = serialShutdownTimeout / 2
			err := cmd.Run()
			if ctx.Err() != nil {
				err = ctx.Err()
			} else if err != nil {
				err = fmt.Errorf("VM console failed: %w: %s", err, stderr.String())
			}
			// A monitor may start before netlaunch powers on the VM.
			if err == nil || !strings.Contains(stderr.String(), "domain is not running") || ctx.Err() != nil {
				writer.CloseWithError(err)
				done <- err
				return
			}
			select {
			case <-ctx.Done():
			case <-ticker.C:
			}
		}
	}()
	stop := context.AfterFunc(ctx, func() { _ = reader.CloseWithError(ctx.Err()) })
	defer stop()
	err := readerLoop(ctx, reader, out, serialLogTailLines)
	cause := ctx.Err()
	cancel()
	reader.Close()
	commandErr := <-done
	if !errors.Is(commandErr, context.Canceled) {
		err = errors.Join(err, commandErr)
	}
	return errors.Join(err, cause)
}

func readerLoop(ctx context.Context, in io.Reader, out io.Writer, ringSize int) error {
	// Create a ring buffer to hold the last N lines of the serial log
	ring := ring.New(ringSize)

	reader := bufio.NewReader(in)
	var lineBuffBuilder strings.Builder
	for {
		// Check for context cancellation
		if ctx.Err() != nil {
			// Print the last `ringSize` lines of the serial log before timing out
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

		// Check if the current line contains the login prompt, and return if it
		// does. The log-in prompt is expected to contain the string "login:"
		// but we need to block the false positive caused by the installer OS
		// login prompt. The installer OS hostname includes the string "mos" so
		// we can use that to filter out installer login prompts.
		if strings.Contains(lineBuffBuilder.String(), "login:") &&
			!strings.Contains(lineBuffBuilder.String(), "mos") {
			logrus.Infof("Login prompt found in VM serial log")
			return nil
		}

		readRune, _, err := reader.ReadRune()
		if errors.Is(err, io.EOF) {
			return fmt.Errorf("VM console ended before the login prompt")
		} else if err != nil {
			return fmt.Errorf("failed to read from serial log file: %w", err)
		}

		if readRune == '\n' {
			// Store the line in the ring buffer
			ring.Value = lineBuffBuilder.String()
			ring = ring.Next()

			// Output the line to the provided writer
			_, err := out.Write([]byte(stormutils.RemoveAllANSI(lineBuffBuilder.String()) + "\n"))
			if err != nil {
				return fmt.Errorf("failed to write serial log output: %w", err)
			}

			// New line, reset line buffer
			lineBuffBuilder.Reset()
		} else {
			// Append rune to line buffer, this operation always succeeds.
			lineBuffBuilder.WriteRune(readRune)
		}
	}
}
