package utils

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/microsoft/storm/pkg/storm/utils"
	"github.com/sirupsen/logrus"
)

func WaitForLoginMessageInSerialLog(vmSerialLog string, verbose bool, iteration int, localSerialLog string) error {
	// Timeout for monitoring serial log for login prompt
	timeout := time.Second * 120
	startTime := time.Now()

	// Wait for serial log
	for {
		if time.Since(startTime) >= timeout {
			return fmt.Errorf("timeout waiting for serial log after %d seconds", int(timeout.Seconds()))
		}
		if _, err := os.Stat(vmSerialLog); err == nil {
			break
		}
	}

	// Create the file if it doesn't exist
	file, err := os.OpenFile(vmSerialLog, os.O_RDWR, 0644)
	if err != nil {
		return fmt.Errorf("failed to open serial log file: %w", err)
	}
	defer file.Close()

	reader := bufio.NewReader(file)
	lineBuffer := ""
	for {
		// Check if the current line contains the login prompt, and return if it does
		if strings.Contains(lineBuffer, "login:") && !strings.Contains(lineBuffer, "mos") {
			printAndSave(lineBuffer, verbose, localSerialLog)
			return nil
		}

		// Read a rune from reader, if EOF is encountered, retry until either a new
		// character is read or the timeout is reached
		var readRune rune
		for {
			if time.Since(startTime) >= timeout {
				return fmt.Errorf("timeout waiting for login prompt after %d seconds", int(timeout.Seconds()))
			}
			// Read a rune from the serial log file
			readRune, _, err = reader.ReadRune()
			if err == io.EOF {
				// Wait for new serial output
				time.Sleep(10 * time.Millisecond)
				continue
			}
			if err != nil {
				return fmt.Errorf("failed to read from serial log: %w", err)
			}
			// Successfully read a rune, break out of the loop
			break
		}
		// Handle the rune read from the serial log
		runeStr := string(readRune)
		if runeStr == "\n" {
			// If the last character is a newline, print the line buffer
			// and reset it
			printAndSave(lineBuffer, verbose, localSerialLog)
			lineBuffer = ""
		} else {
			// If non-newline, append the output to the buffer
			lineBuffer += runeStr
		}
	}
}

func printAndSave(line string, verbose bool, localSerialLog string) {
	if line == "" {
		return
	}

	// Remove ANSI control codes
	line = utils.ANSI_CONTROL_CLEANER.ReplaceAllString(line, "")
	if verbose {
		logrus.Info(line)
	}
	if localSerialLog != "" {
		// Remove all ANSI escape codes
		line = utils.ANSI_CLEANER.ReplaceAllString(line, "")
		logFile, err := os.OpenFile(localSerialLog, os.O_APPEND|os.O_CREATE|os.O_RDWR, 0644)
		if err != nil {
			return
		}
		defer logFile.Close()

		_, err = logFile.WriteString(line + "\n")
		if err != nil {
			logrus.Errorf("Failed to append line to output file: %v", err)
		}
	}
}
