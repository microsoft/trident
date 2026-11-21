package acl

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

func TestWaitForBootCandidatesSkipsStalledLease(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	started := make(chan struct{})
	stale := &testSSHConn{closed: make(chan struct{})}
	live := &ssh.Client{Conn: &testSSHConn{}}
	defer live.Close()
	boot, err := waitForBootCandidates(ctx, []string{"stale", "live", "live"}, func(ctx context.Context, ip string) (*ssh.Client, string, error) {
		if ip == "stale" {
			close(started)
			<-ctx.Done()
			return &ssh.Client{Conn: stale}, "", ctx.Err()
		}
		<-started
		return live, "new-boot", nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if boot.ip != "live" || boot.id != "new-boot" || boot.client != live {
		t.Fatalf("unexpected accepted boot: %+v", boot)
	}
	select {
	case <-stale.closed:
	case <-time.After(time.Second):
		t.Fatal("losing probe was not canceled and closed")
	}
}

func TestWaitForBootCandidatesClosesExtraSuccess(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	connections := map[string]*testSSHConn{
		"a": {closed: make(chan struct{})},
		"b": {closed: make(chan struct{})},
	}
	var started atomic.Int32
	ready := make(chan struct{})
	boot, err := waitForBootCandidates(ctx, []string{"a", "b"}, func(_ context.Context, stringIP string) (*ssh.Client, string, error) {
		if started.Add(1) == 2 {
			close(ready)
		}
		<-ready
		return &ssh.Client{Conn: connections[stringIP]}, "new-boot", nil
	})
	if err != nil {
		t.Fatal(err)
	}
	defer boot.client.Close()
	for ip, conn := range connections {
		if ip == boot.ip {
			if conn.closes.Load() != 0 {
				t.Fatal("winning connection was closed")
			}
			continue
		}
		select {
		case <-conn.closed:
		case <-time.After(time.Second):
			t.Fatal("extra successful connection leaked")
		}
	}
}

func TestWaitForBootCandidatesReportsFailureAndDeadline(t *testing.T) {
	failure := errors.New("guest is not ready")
	_, err := waitForBootCandidates(context.Background(), []string{"a", "b"}, func(context.Context, string) (*ssh.Client, string, error) {
		return nil, "", failure
	})
	if !errors.Is(err, failure) {
		t.Fatalf("lost probe failure: %v", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	_, err = waitForBootCandidates(ctx, []string{"stalled"}, func(ctx context.Context, _ string) (*ssh.Client, string, error) {
		<-ctx.Done()
		return nil, "", ctx.Err()
	})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("lost boot deadline: %v", err)
	}
}
