package scenario

import (
	"context"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tridenttools/storm/utils/sshutils"

	"golang.org/x/crypto/ssh"
)

func TestCaptureServiceLogPreservesOutputAndExitStatus(t *testing.T) {
	for _, status := range []uint32{0, 1} {
		t.Run(fmt.Sprint(status), func(t *testing.T) {
			client := journalTestClient(t, status)
			broker := failureTestBroker{t: t, dir: t.TempDir()}
			err := captureServiceLog(client, broker, "docker.service")
			if (err != nil) != (status != 0) {
				t.Fatalf("status %d produced error %v", status, err)
			}
			data, err := os.ReadFile(filepath.Join(broker.dir, "container/docker.service.log"))
			if err != nil {
				t.Fatal(err)
			}
			for _, text := range []string{"journal output", "journal stderr"} {
				if !strings.Contains(string(data), text) {
					t.Fatalf("lost %q: %s", text, data)
				}
			}
		})
	}
}

func journalTestClient(t *testing.T, status uint32) *ssh.Client {
	t.Helper()
	key, _, err := sshutils.GenerateRsaKeyPair(2048)
	if err != nil {
		t.Fatal(err)
	}
	signer, err := ssh.ParsePrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	config := &ssh.ServerConfig{NoClientAuth: true}
	config.AddHostKey(signer)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { listener.Close() })

	done := make(chan struct{})
	go func() {
		defer close(done)
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		server, channels, requests, err := ssh.NewServerConn(conn, config)
		if err != nil {
			t.Error(err)
			return
		}
		defer server.Close()
		go ssh.DiscardRequests(requests)
		for incoming := range channels {
			channel, requests, err := incoming.Accept()
			if err != nil {
				t.Error(err)
				return
			}
			for request := range requests {
				if request.Type != "exec" {
					request.Reply(false, nil)
					continue
				}
				var command struct{ Command string }
				if err := ssh.Unmarshal(request.Payload, &command); err != nil {
					t.Error(err)
				}
				if !strings.Contains(command.Command, "journalctl --no-pager -u 'docker.service'") {
					t.Errorf("unexpected command: %s", command.Command)
				}
				request.Reply(true, nil)
				io.WriteString(channel, "journal output\n")
				io.WriteString(channel.Stderr(), "journal stderr\n")
				channel.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{status}))
				channel.Close()
				break
			}
		}
	}()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	client, err := sshutils.CreateSshClient(ctx, sshutils.SshClientConfig{
		User:       "test",
		Host:       "127.0.0.1",
		Port:       uint16(listener.Addr().(*net.TCPAddr).Port),
		PrivateKey: key,
	})
	if err != nil {
		t.Fatal(err)
	}
	// Dial cancellation must not close a client already returned to its caller.
	cancel()
	t.Cleanup(func() {
		client.Close()
		<-done
	})
	return client
}
