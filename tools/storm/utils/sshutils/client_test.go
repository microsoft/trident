package sshutils

import (
	"context"
	"net"
	"testing"
	"time"

	stormssh "github.com/microsoft/storm/utils/ssh"
)

func TestCreateSshClientCancelsStalledHandshake(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	serverCtx, stopServer := context.WithCancel(context.Background())
	serverDone := make(chan struct{})
	go func() {
		defer close(serverDone)
		conn, err := listener.Accept()
		if err == nil {
			defer conn.Close()
			<-serverCtx.Done()
		}
	}()
	defer func() {
		stopServer()
		listener.Close()
		<-serverDone
	}()

	pair, err := stormssh.GenerateKey()
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	result := make(chan error, 1)
	go func() {
		client, err := CreateSshClient(ctx, SshClientConfig{
			User:       "test",
			Host:       "127.0.0.1",
			Port:       uint16(listener.Addr().(*net.TCPAddr).Port),
			PrivateKey: pair.PEM,
		})
		if client != nil {
			client.Close()
		}
		result <- err
	}()
	select {
	case err := <-result:
		if err == nil {
			t.Fatal("expected the stalled handshake to fail")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("SSH handshake ignored context cancellation")
	}
}
