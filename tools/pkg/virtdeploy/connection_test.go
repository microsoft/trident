package virtdeploy

import (
	"context"
	"encoding/binary"
	"errors"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/digitalocean/go-libvirt/libvirttest"
)

type stalledLibvirtConn struct {
	net.Conn
	stallAt  int
	requests int
	reached  chan struct{}
	once     sync.Once
}

func (c *stalledLibvirtConn) Write(data []byte) (int, error) {
	if len(data) >= 28 && binary.BigEndian.Uint32(data[4:8]) == 0x20008086 {
		c.requests++
	}
	if c.requests >= c.stallAt {
		c.once.Do(func() { close(c.reached) })
		return len(data), nil
	}
	return c.Conn.Write(data)
}

func TestLibvirtHandshakeCancellation(t *testing.T) {
	mock := libvirttest.New()
	raw, err := mock.Dial()
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	conn := &stalledLibvirtConn{Conn: raw, stallAt: 1, reached: make(chan struct{})}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	result := make(chan error, 1)
	go func() {
		_, _, err := connectWithConn(ctx, conn)
		result <- err
	}()
	select {
	case <-conn.reached:
	case <-time.After(3 * time.Second):
		t.Fatal("handshake was never reached")
	}
	cancel()
	select {
	case err := <-result:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("handshake cancellation was lost: %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("handshake survived cancellation")
	}
}

func TestLibvirtConnectionOutlivesDialContext(t *testing.T) {
	mock := libvirttest.New()
	raw, err := mock.Dial()
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	ctx, cancel := context.WithCancel(context.Background())
	lv, closeConn, err := connectWithConn(ctx, raw)
	if err != nil {
		t.Fatal(err)
	}
	defer closeConn()
	cancel()
	if _, err := lv.ConnectGetLibVersion(); err != nil {
		t.Fatalf("successful connection retained dial cancellation: %v", err)
	}
}

func TestResourceCleanupCancelsStalledRPCs(t *testing.T) {
	for _, stage := range []struct {
		name    string
		stallAt int
	}{{"domains", 3}, {"networks", 4}} {
		t.Run(stage.name, func(t *testing.T) {
			mock := libvirttest.New()
			raw, err := mock.Dial()
			if err != nil {
				t.Fatal(err)
			}
			defer raw.Close()
			conn := &stalledLibvirtConn{Conn: raw, stallAt: stage.stallAt, reached: make(chan struct{})}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			lv, closeConn, err := connectWithConn(ctx, conn)
			if err != nil {
				t.Fatal(err)
			}
			result := make(chan error, 1)
			go func() { result <- cleanupResources(ctx, "unmatched-test-namespace", lv, closeConn) }()
			select {
			case <-conn.reached:
			case <-time.After(3 * time.Second):
				t.Fatal("cleanup RPC was never reached")
			}
			cancel()
			select {
			case err := <-result:
				if !errors.Is(err, context.Canceled) {
					t.Fatalf("cleanup cancellation was lost: %v", err)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("cleanup survived cancellation")
			}
		})
	}
}

func TestLibvirtSocketCloseDoesNotSendRPC(t *testing.T) {
	mock := libvirttest.New()
	raw, err := mock.Dial()
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	conn := &stalledLibvirtConn{Conn: raw, stallAt: 3, reached: make(chan struct{})}
	_, closeConn, err := connectWithConn(context.Background(), conn)
	if err != nil {
		t.Fatal(err)
	}
	result := make(chan error, 1)
	go func() { result <- closeConn() }()
	select {
	case err := <-result:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("socket close waited for a libvirt RPC")
	}
	select {
	case <-conn.reached:
		t.Fatal("socket close sent an RPC")
	default:
	}
}

func TestResourceCleanupDeadline(t *testing.T) {
	mock := libvirttest.New()
	raw, err := mock.Dial()
	if err != nil {
		t.Fatal(err)
	}
	defer raw.Close()
	conn := &stalledLibvirtConn{Conn: raw, stallAt: 3, reached: make(chan struct{})}
	lv, closeConn, err := connectWithConn(context.Background(), conn)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	result := make(chan error, 1)
	go func() { result <- cleanupResources(ctx, "unmatched-test-namespace", lv, closeConn) }()
	select {
	case err := <-result:
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("cleanup deadline was lost: %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("cleanup exceeded its deadline")
	}
}
