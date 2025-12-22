package scenario

import (
	"bufio"
	"container/ring"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/digitalocean/go-libvirt"
	"github.com/microsoft/storm/pkg/storm/utils"
	"github.com/sirupsen/logrus"

	"tridenttools/pkg/ref"
	fileutils "tridenttools/storm/utils/file"
)

// spawnVMSerialLogger starts the VM serial logger for the test host IF it is a
// VM, and logs the output. This function will attempt to delete any
// pre-existing log file before starting the logger to ensure a clean log, so IT
// MUST BE CALLED BEFORE THE VM IS STARTED.
//
// The output of the serial logger will be written to the provided
// io.WriteCloser. The WriteCloser will be closed when the logger exits.
//
// If the hardware type is not VM, this function is a no-op and returns nil.
//
// If an error occurs while starting the logger, that error is returned.
//
// The returned channel can be used to wait for the logger to finish by waiting
// on it for a value. The channel will be closed once the logger has finished.
// If the logger doesn't run (because the hardware type is not VM), the channel will be
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
			logrus.Warnf("######################## Waiting for VM serial monitor to finish...")
			wg.Wait()
			doneChannel <- true
			logrus.Warnf("######################## VM serial monitor finished, channel notified.")
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

	// serialLogPath, err := vmInfo.SerialLogPath()
	// if err != nil {
	// 	return doneChannel, fmt.Errorf("failed to get serial log path: %w", err)
	// }

	// exists, err := file.FileExists(serialLogPath)
	// if err != nil {
	// 	return doneChannel, fmt.Errorf("failed to check if serial log file exists: %w", err)
	// }
	// if exists {
	// 	// Delete pre-existing serial log file to ensure a clean log
	// 	logrus.Warnf("######################## VM serial monitor deleting pre-existing serial log file: %s", serialLogPath)
	// 	err := os.Remove(serialLogPath)
	// 	if err != nil {
	// 		return doneChannel, fmt.Errorf("failed to delete pre-existing VM serial log file: %w", err)
	// 	}
	// }

	// wg.Add(1)
	// go func() {
	// 	defer wg.Done()
	// 	defer output.Close()
	// 	logrus.Warnf("######################## Starting VM serial monitor...")
	// 	err := waitForVmSerialLogLogin(ctx, serialLogPath, output)
	// 	logrus.Warnf("######################## VM serial monitor ended...")
	// 	if err != nil {
	// 		if errors.Is(err, fs.ErrPermission) {
	// 			err = fmt.Errorf("permission denied when accessing VM serial log file (are you missing sudo?): %w", err)
	// 		}
	// 		errStr := fmt.Sprintf("VM serial log monitor ended with error: %v", err)
	// 		logrus.Error(errStr)
	// 		output.Write([]byte(fmt.Sprintf("ERROR: %s", errStr)))
	// 	} else {
	// 		logrus.Infof("VM serial log monitor ended successfully")
	// 	}
	// }()

	wg.Add(1)
	go func() {
		defer wg.Done()
		defer output.Close()
		logrus.Warnf("######################## Starting VM serial monitor...")
		err := waitForVmSerialLogLoginLibvirt(ctx, vmInfo.Lv(), vmInfo.LvDomain(), output)
		logrus.Warnf("######################## VM serial monitor ended...")
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

// Wait for the VM serial log file to be created.
//
// This function will block until the serial log file is created or the timeout
// is reached.
//
// If the log file never appears, this indicates an infrastructure error and the test
// case will be marked as errored.
//
// If the login prompt never appears, this indicates a problem with the VM booting
// and the test case will be marked as failed.
func waitForVmSerialLogLogin(ctx context.Context, vmSerialLog string, out io.Writer) error {
	// Create a ring buffer to hold the last 10 lines of the serial log
	ringSize := 25
	ring := ring.New(ringSize)

	// Wait for serial log file to exist
	logrus.Warnf("######################## VM serial monitor waiting for serial log file to be created...")
	err := fileutils.WaitForFileToExist(ctx, vmSerialLog)
	if err != nil {
		// If the file was never created there is an infra error
		return fmt.Errorf("failed to find VM serial log file: %w", err)
	}

	logrus.WithField("serialLog", vmSerialLog).Debugf("Serial log file found, starting serial monitor...")

	// Open serial log file for reading
	file, err := os.Open(vmSerialLog)
	if err != nil {
		return fmt.Errorf("failed to open serial log file: %w", err)
	}
	defer file.Close()

	reader := bufio.NewReader(file)
	lineBuffer := ""
	for {
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
			fmt.Fprintf(out, "EOF on %s - %d\n", file.Name(), file.Fd())
			continue
		} else if err != nil {
			return fmt.Errorf("failed to read from serial log file: %w", err)
		}

		fmt.Fprintf(out, "%c", readRune)

		runeStr := string(readRune)
		if runeStr == "\n" {
			// Store the line in the ring buffer
			ring.Value = lineBuffer
			ring = ring.Next()

			// Output the line to the provided writer
			// out.Write([]byte(utils.RemoveAllANSI(lineBuffer) + "\n"))
			// New line, reset line buffer
			lineBuffer = ""
		} else {
			// Append rune to line buffer
			lineBuffer += runeStr
		}
	}
}

func waitForVmSerialLogLoginLibvirt(ctx context.Context, lv *libvirt.Libvirt, dom libvirt.Domain, out io.Writer) error {
	// Create a ring buffer to hold the last 10 lines of the serial log
	ringSize := 25
	ring := ring.New(ringSize)

	logrus.Warnf("######################## VM serial monitor starting libvirt serial console reader...")

	pr, pw := io.Pipe()
	defer pr.Close()
	defer pw.Close()

	consoleCtx, consoleCancel := context.WithCancel(ctx)
	defer consoleCancel()

	var wg sync.WaitGroup
	defer wg.Wait()

	// DomainOpenConsole blocks until the console stream ends.
	errCh := make(chan error, 1)
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer pw.Close()
		for {
			if consoleCtx.Err() != nil {
				return
			}
			err := lv.DomainOpenConsole(dom, nil, pw, 0)
			if err != nil {
				logrus.Errorf("Error opening libvirt domain console: %v", err)
				// If the pipe is closed or context cancelled, exit the
				// goroutine. Otherwise, retry forever.
				if errors.Is(err, io.ErrClosedPipe) || consoleCtx.Err() != nil {
					errCh <- err
					return
				}
				time.Sleep(100 * time.Millisecond)
			}
		}
	}()

	reader := bufio.NewReader(pr)
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
				return fmt.Errorf("libvirt console stream ended with error: %w", err)
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
