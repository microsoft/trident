package libvirtutils

import (
	"bufio"
	"container/ring"
	"context"
	"errors"
	"fmt"
	"io"
	"strings"
	"sync"
	"time"

	"github.com/digitalocean/go-libvirt"
	stormutils "github.com/microsoft/storm/pkg/storm/utils"
	"github.com/sirupsen/logrus"

	ioutils "tridenttools/storm/utils/io"
)

func WaitForVmSerialLogLoginLibvirt(ctx context.Context, lv *libvirt.Libvirt, domain libvirt.Domain, out io.Writer) error {
	pipeReader, pipeWriter := io.Pipe()
	consoleCtx, consoleCancel := context.WithCancel(ctx)
	var wg sync.WaitGroup

	// Context to track whether the console is open. This is used to cancel the
	// reader loop if the console goroutine exits, because this means there will
	// never be any more data to read, so we should exit the reader loop as
	// well.
	consoleIsOpenCtx, consoleIsOpenCancel := context.WithCancel(ctx)
	defer consoleIsOpenCancel()

	// Spawn DomainOpenConsole in a goroutine because it's a blocking call.
	errCh := make(chan error, 1)
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer consoleIsOpenCancel()
		defer pipeWriter.Close()
		pipeNotifyWriter := ioutils.NewNotifyWriter(pipeWriter)
		for {
			// If context is cancelled, exit the goroutine.
			if consoleCtx.Err() != nil {
				return
			}

			// Try to open the console. This is a blocking call that only
			// returns when the console is closed or an error occurs. It writes
			// to the provided writer in the background.
			err := lv.DomainOpenConsole(domain, nil, pipeNotifyWriter, uint32(libvirt.DomainConsoleForce))
			if err == nil && pipeNotifyWriter.Active() {
				// DomainOpenConsole returned without error and data was
				// written, this is an expected outcome when the console closed
				// naturally.
				return
			}

			if consoleCtx.Err() != nil {
				// Context was cancelled while the console was open/opening,
				// exit the goroutine.
				return
			}

			if !pipeNotifyWriter.Active() {
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
	loopErr := readerLoop(consoleIsOpenCtx, pipeReader, errCh, out, 30)
	// Regardless of whether readerLoop returned an error, cancel the console
	// context and close the pipe to stop the DomainOpenConsole goroutine.
	consoleCancel()
	pipeReader.Close()
	pipeWriter.Close()

	// Even after we close all of this, the DomainOpenConsole goroutine may
	// still be running because it doesn't take in a context. We force it to
	// close by opening a new console with the DomainConsoleForce flag, and a
	// nil stream, which will signal the existing DomainOpenConsole to exit, and
	// make this new one exit immediately.
	err := lv.DomainOpenConsole(domain, nil, nil, uint32(libvirt.DomainConsoleForce))
	if err != nil {
		logrus.Warnf("failed to force close DomainOpenConsole: %v", err)
	}

	// Wait for DomainOpenConsole goroutine to exit
	wg.Wait()

	return loopErr
}

func readerLoop(ctx context.Context, in io.Reader, errCh <-chan error, out io.Writer, ringSize int) error {
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

		// Check if the console stream has ended
		select {
		case err := <-errCh:
			if err != nil {
				err = fmt.Errorf("libvirt console stream ended with error: %w", err)
			}
			logrus.Infof("Libvirt console stream ended")
			return err
		default:
			// Continue reading
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
