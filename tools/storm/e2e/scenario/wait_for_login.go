package scenario

import (
	"bufio"
	"container/ring"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/microsoft/storm"
	"github.com/microsoft/storm/pkg/storm/utils"
	"github.com/sirupsen/logrus"

	fileutils "tridenttools/storm/utils/file"
)

// Watch VM serial log and wait for login prompt to appear.
func (s *TridentE2EScenario) waitForLoginVm(tc storm.TestCase) error {
	// Double check that this is a VM scenario
	if s.hardware != HardwareTypeVM {
		tc.Skip("not a VM test scenario")
	}

	vmInfo := s.testHost.VmInfo()
	if vmInfo == nil {
		return fmt.Errorf("test host VM info is nil")
	}

	vmSerialLog, err := vmInfo.SerialLogPath()
	if err != nil {
		tc.Error(err)
	}

	// Wait for login prompt in the serial log
	logrus.Infof("Waiting for login prompt in VM serial log...")

	// err := stormutils.WaitForLoginMessageInSerialLog(vmSerialLog, true, 1, fmt.Sprintf("%s/serial.log", h.args.ArtifactsFolder), time.Minute*5)
	// if err != nil {
	// 	tc.FailFromError(err)
	// 	return err
	// }

	file, err := os.CreateTemp("", "")
	if err != nil {
		return fmt.Errorf("failed to create temporary file for serial log output: %w", err)
	}
	defer os.Remove(file.Name())

	err = waitForVmSerialLogLogin(tc, vmSerialLog, time.Duration(s.args.VmWaitForLoginTimeout)*time.Second, file)
	if err != nil {
		return err
	}

	tc.ArtifactBroker().PublishArtifact("wait-for-login", file.Name())

	return nil
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
func waitForVmSerialLogLogin(tc storm.TestCase, vmSerialLog string, timeout time.Duration, out io.Writer) error {
	// Create a context that timeouts after VmWaitForLoginTimeout seconds
	ctx, cancel := context.WithTimeout(tc.Context(), timeout)
	defer cancel()

	// Create a ring buffer to hold the last 10 lines of the serial log
	ringSize := 20
	ring := ring.New(ringSize)

	// Wait for serial log file to exist
	err := fileutils.WaitForFileToExist(ctx, vmSerialLog)
	if err != nil {
		// If the file was never created there is an infra error
		return fmt.Errorf("failed to find VM serial log file: %w", err)
	}

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
			logrus.Errorf("Last {} lines of VM serial log before timeout:\n", ringSize, func() string {
				var sb strings.Builder
				ring.Do(func(p interface{}) {
					if p != nil {
						sb.WriteString(p.(string) + "\n")
					}
				})
				return sb.String()
			}())

			tc.Fail("timed out waiting for login prompt in serial log")
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
			tc.Error(fmt.Errorf("failed to read from serial log: %w", err))
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
