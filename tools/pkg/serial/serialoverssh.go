package serial

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"os"
	"regexp"
	"time"

	log "github.com/sirupsen/logrus"
	"golang.org/x/crypto/ssh"
)

type SerialOverSSHSettings struct {
	Host     string
	Port     uint16
	Username string
	Password string
	ComPort  string
	Output   string
}

type SerialOverSshSession struct {
	client    *ssh.Client
	session   *ssh.Session
	file      *os.File
	done      chan bool
	cancel    *context.CancelFunc
	ssh_stdin *io.WriteCloser
}

func (s *SerialOverSshSession) Close() {
	if s.cancel != nil {
		(*s.cancel)()
	}

	// Wait for the writer goroutine to finish
	<-s.done
	close(s.done)

	if s.ssh_stdin != nil {
		(*s.ssh_stdin).Close()
	}

	if s.session != nil {
		s.session.Close()
	}
	if s.client != nil {
		s.client.Close()
	}
	if s.file != nil {
		s.file.Sync()
		s.file.Close()
	}
}

func (s *SerialOverSshSession) start(comPort string) error {
	command := fmt.Sprintf("console %s", comPort)

	// Open the Stdout, Stderr, and Stdin pipes
	stdout_p, err := s.session.StdoutPipe()
	if err != nil {
		log.WithError(err).Error("Serial: Failed to open stdout pipe")
	}

	stderr_p, err := s.session.StderrPipe()
	if err != nil {
		log.WithError(err).Error("Serial: Failed to open stderr pipe")
	}

	stdin_p, err := s.session.StdinPipe()
	if err != nil {
		log.WithError(err).Error("Serial: Failed to open stdin pipe")
	}
	// The BMC is happier if we keep the stdin open, it will not
	// stream the output if we close it.
	s.ssh_stdin = &stdin_p

	// Start the command
	err = s.session.Start(command)
	if err != nil {
		log.WithError(err).Error("Serial: Failed to start serial console on SSH session")
		return err
	}
	log.Info("Serial: Started serial over SSH")

	// Create buffered readers for the stdout and stderr
	outbuf := bufio.NewReader(stdout_p)
	errbuf := bufio.NewReader(stderr_p)

	// Context for canceling the goroutines
	ctx, cancel := context.WithCancel(context.Background())
	s.cancel = &cancel

	// Channel to collect the output from the serial console
	output := make(chan []byte)

	// Fetch function to read from the buffer and write to the output channel.
	// This goroutine will run run forever in the background until main
	// finishes. This is because Reading is a blocking call and we need to read
	// from the buffer continuously. To mitigate the issues of ungracefully killing
	// the goroutine, we use a channel to send each line into a writer goroutine
	// which will write to the file and has a graceful exit. At worst, we will
	// lose the last (incomplete)line of the output.
	fetch := func(name string, buffer *bufio.Reader) {
		for {
			select {
			case <-ctx.Done():
				// Cancel the goroutine. This will likely never run as ReadBytes
				// is a blocking call so we will be there most of the time. But
				// at least we can try.
				return
			default:
				// Read full line, unfortunately a blocking call.
				line, err := buffer.ReadBytes('\n')
				if err == nil {
					// On success, send the line to the output channel.
					output <- line
				} else {
					if err == io.EOF {
						// Ge got EOF, which means there is nothing more to read for
						// now. Sleep for a bit to allow the last line to be
						// written.
						time.Sleep(100 * time.Millisecond)
					} else {
						// Some other error occurred, log it and break the loop.
						log.WithError(err).WithField("buffer", name).Error("Serial: Failed to read from buffer")
						break
					}
				}
			}
		}
	}

	// Start the writer goroutine
	go func() {
		// Regex to clean al ANSI control sequences (does not match coloring)
		ansi_control := regexp.MustCompile(`(\x9B|\x1B\[)[0-?]*[ -\/]*[@-ln-~]`)
		for {
			select {

			// Write the output to the file
			case line := <-output:
				// Clean the line from ANSI control sequences
				line = ansi_control.ReplaceAll(line, []byte(""))

				// Skip empty lines
				if string(line) == "" {
					continue
				}

				_, err := s.file.Write(line)
				if err != nil {
					log.WithError(err).Error("Serial: Failed to write to output file")
				}

			// Cancel the goroutines
			case <-ctx.Done():
				log.Debug("Serial: Closing writer goroutine")
				s.done <- true
				return
			}
		}
	}()

	// Start the fetch goroutines for stdout and stderr
	go fetch("stdout", outbuf)
	go fetch("stderr", errbuf)

	return nil
}

func NewSerialOverSshSession(config SerialOverSSHSettings) (*SerialOverSshSession, error) {
	ssh_config := &ssh.ClientConfig{
		User: config.Username,
		Auth: []ssh.AuthMethod{
			ssh.Password(config.Password),
			ssh.KeyboardInteractive(func(name, instruction string, questions []string, echos []bool) ([]string, error) {
				// Just answer all questions with the password
				answers := make([]string, len(questions))
				for i := range answers {
					answers[i] = config.Password
				}
				return answers, nil
			}),
		},
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         time.Second * 15,
	}

	sshOverSerial := SerialOverSshSession{
		done: make(chan bool),
	}

	address := fmt.Sprintf("%s:%d", config.Host, config.Port)
	log.WithField("address", address).Debug("Serial: Dialing BMC over SSH")
	client, err := ssh.Dial("tcp", address, ssh_config)
	if err != nil {
		log.WithError(err).Error("Serial: Failed to dial SSH")
		return nil, err
	}
	sshOverSerial.client = client

	log.Debug("Serial: Creating BMC SSH session")
	session, err := client.NewSession()
	if err != nil {
		sshOverSerial.Close()
		log.WithError(err).Error("Serial: Failed to create SSH session")
		return nil, err
	}
	sshOverSerial.session = session

	outfile, err := os.Create(config.Output)
	if err != nil {
		sshOverSerial.Close()
		log.WithError(err).Error("Serial: Failed to create output file")
		return nil, err
	}
	sshOverSerial.file = outfile
	log.WithField("output", outfile.Name()).Debug("Serial: Writing output to file")

	err = sshOverSerial.start(config.ComPort)
	if err != nil {
		sshOverSerial.Close()
		log.WithError(err).Error("Serial: Failed to start serial console")
		return nil, err
	}

	return &sshOverSerial, nil
}
