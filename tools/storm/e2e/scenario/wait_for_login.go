package scenario

import (
	"bufio"
	"container/ring"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"strings"
	"sync"
	"time"

	"github.com/digitalocean/go-libvirt"
	"github.com/microsoft/storm/pkg/storm/utils"
	"github.com/sirupsen/logrus"

	"tridenttools/pkg/ref"
	ioutils "tridenttools/storm/utils/io"
)

// spawnVMSerialLogger starts the VM serial logger for the test host IF it is a
// VM.
//
// The output of the serial logger will be written live to the provided
// io.WriteCloser. The WriteCloser will be closed when the logger exits.
//
// If the hardware type is not VM, this function is a no-op and returns nil.
//
// If an error occurs while starting the logger, that error is returned.
//
// The returned channel can be used to wait for the logger to finish by waiting
// on it for a value. The channel will be closed once the logger has finished.
// If the logger doesn't run (because the hardware type is not VM), the channel
// will receive a value immediately.
//
// The logger will run until the context is cancelled or the login prompt is
// detected.
func (s *TridentE2EScenario) spawnVMSerialMonitor(ctx context.Context, output io.WriteCloser) (<-chan bool, error) {
	doneChannel := make(chan bool)
	var wg sync.WaitGroup

	// On exit, wait for the waitgroup to finish and then send a value on the
	// done channel and close it.
	defer func() {
		go func() {
			wg.Wait()
			doneChannel <- true
			close(doneChannel)
		}()
	}()

	// Only spawn the VM serial logger if the hardware type is VM. Otherwise, do
	// nothing.
	if s.hardware != HardwareTypeVM {
		return doneChannel, nil
	}

	// Get VM info
	vmInfo := s.testHost.VmInfo()
	if ref.IsNilInterface(vmInfo) {
		return doneChannel, fmt.Errorf("vm host info not set")
	}

	wg.Add(1)
	go func() {
		defer wg.Done()
		defer output.Close()
		err := waitForVmSerialLogLoginLibvirt(ctx, vmInfo.Lv(), vmInfo.LvDomain(), output)
		if err != nil {
			if errors.Is(err, fs.ErrPermission) {
				err = fmt.Errorf("permission denied when accessing VM serial log file (are you missing sudo?): %w", err)
			}
			errStr := fmt.Sprintf("VM serial log monitor ended with error: %v", err)
			logrus.Error(errStr)
			output.Write([]byte(fmt.Sprintf("ERROR: %s", errStr)))
		} else {
			logrus.Infof("VM serial log monitor ended successfully")
		}
	}()

	return doneChannel, nil
}

func waitForVmSerialLogLoginLibvirt(ctx context.Context, lv *libvirt.Libvirt, dom libvirt.Domain, out io.Writer) error {

	pr, pw := io.Pipe()

	consoleCtx, consoleCancel := context.WithCancel(ctx)

	var wg sync.WaitGroup

	// Spawn DomainOpenConsole in a goroutine because it's a blocking call.
	errCh := make(chan error, 1)
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer pw.Close()
		wN := ioutils.NewNotifyWriter(pw)
		for {
			// If context is cancelled, exit the goroutine.
			if consoleCtx.Err() != nil {
				return
			}

			// Try to open the console.
			err := lv.DomainOpenConsole(dom, nil, wN, 0)
			if err == nil {
				// DomainOpenConsole returned without error, exit the goroutine.
				if wN.Active {
					// Data was written, this is expected.
					return
				} else {
					// No data was written, this is unexpected. Log a warning.
					time.Sleep(100 * time.Millisecond)
					continue
				}
			}

			if consoleCtx.Err() != nil {
				// Context was cancelled while the console was open/opening,
				// exit the goroutine.
				return
			}

			if !wN.Active {
				// No data has been written yet, so this is likely a
				// transient error such as the domain not being fully
				// started yet. Retry silently.
				time.Sleep(100 * time.Millisecond)
				continue
			}

			// If we get here, there was an error after some data was
			// successfully written. Log the error and exit the goroutine. This
			// may happen naturally when the pipe is closed. But if that happens
			// naturally, the readerLoop will have exited first, so this error
			// won't matter.
			logrus.Errorf("DomainOpenConsole error after data written: %v", err)
			errCh <- err
			return
		}
	}()

	// Call inner loop
	err := readerLoop(ctx, pr, errCh, out, 30)
	// Regardless of whether readerLoop returned an error, cancel the console
	// context and close the pipe to stop the DomainOpenConsole goroutine.
	consoleCancel()
	pr.Close()
	pw.Close()

	// Wait for DomainOpenConsole goroutine to exit
	wg.Wait()

	return err
}

func readerLoop(ctx context.Context, in io.Reader, errCh <-chan error, out io.Writer, ringSize int) error {
	// Create a ring buffer to hold the last N lines of the serial log
	ring := ring.New(ringSize)

	reader := bufio.NewReader(in)
	lineBuffer := ""
	for {
		// Check for context cancellation
		if ctx.Err() != nil {
			// Print the last 10 lines of the serial log before timing out
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

		// Check if the console stream has ended
		select {
		case err := <-errCh:
			if err != nil {
				err = fmt.Errorf("libvirt console stream ended with error: %w", err)
			}
			logrus.Infof("Libvirt console stream ended")
			return nil
		default:
			// Continue reading
		}

		// Check if the current line contains the login prompt, and return if it does
		if strings.Contains(lineBuffer, "login:") && !strings.Contains(lineBuffer, "mos") {
			logrus.Infof("Login prompt found in VM serial log")
			return nil
		}

		// Read a rune from reader, if EOF is encountered, retry until either a new
		// character is read or the timeout is reached
		readRune, _, err := reader.ReadRune()
		if errors.Is(err, io.EOF) {
			// Wait for new serial output
			time.Sleep(100 * time.Millisecond)
			continue
		} else if err != nil {
			return fmt.Errorf("failed to read from serial log file: %w", err)
		}

		runeStr := string(readRune)
		if runeStr == "\n" {
			// Store the line in the ring buffer
			ring.Value = lineBuffer
			ring = ring.Next()

			// Output the line to the provided writer
			out.Write([]byte(utils.RemoveAllANSI(lineBuffer) + "\n"))
			// New line, reset line buffer
			lineBuffer = ""
		} else {
			// Append rune to line buffer
			lineBuffer += runeStr
		}
	}
}
