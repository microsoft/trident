package client

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"io"
	"net"
	"strings"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

func testSSH(t *testing.T, run func(ssh.Channel)) *ssh.Client {
	t.Helper()
	_, key, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	signer, err := ssh.NewSignerFromKey(key)
	if err != nil {
		t.Fatal(err)
	}
	config := &ssh.ServerConfig{NoClientAuth: true}
	config.AddHostKey(signer)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	go func() {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		server, channels, requests, err := ssh.NewServerConn(conn, config)
		if err != nil {
			return
		}
		defer server.Close()
		go ssh.DiscardRequests(requests)
		for channel := range channels {
			stream, requests, err := channel.Accept()
			if err != nil {
				return
			}
			go func() {
				defer stream.Close()
				for request := range requests {
					if request.Type == "exec" {
						_ = request.Reply(true, nil)
						run(stream)
						return
					}
					_ = request.Reply(false, nil)
				}
			}()
		}
	}()
	client, err := ssh.Dial("tcp", listener.Addr().String(), &ssh.ClientConfig{User: "test", HostKeyCallback: ssh.InsecureIgnoreHostKey()})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = client.Close() })
	return client
}

func TestRunCommandDrainsSeparateStreamsAndRetainsExitStatus(t *testing.T) {
	stderr := strings.Repeat("e", 4*1024*1024)
	client := testSSH(t, func(channel ssh.Channel) {
		_, _ = io.WriteString(channel.Stderr(), stderr)
		_, _ = io.WriteString(channel, "command stdout")
		_, _ = channel.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{17}))
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	out, err := RunCommandContext(ctx, client, "test")
	if err != nil {
		t.Fatal(err)
	}
	if out.Stdout != "command stdout" || out.Stderr != stderr || out.Status != 17 {
		t.Fatalf("incorrect streams/status: stdout=%q stderr bytes=%d status=%d", out.Stdout, len(out.Stderr), out.Status)
	}
}

func TestRunCommandCancellationClosesBlockedSession(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	client := testSSH(t, func(channel ssh.Channel) { <-release })
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	_, err := RunCommandContext(ctx, client, "blocked")
	if err != context.DeadlineExceeded {
		t.Fatalf("expected deadline, got %v", err)
	}
}
