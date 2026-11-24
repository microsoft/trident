package qemu

import (
	"context"
	"encoding/binary"
	"errors"
	"net"
	"testing"
	"time"

	"github.com/digitalocean/go-libvirt"
	"github.com/digitalocean/go-libvirt/libvirttest"
)

// Drop one request and its payload so the client waits forever for a reply.
type blackholeLibvirtConn struct {
	net.Conn
	stallAt, requests int
	stalled           bool
	reached           chan struct{}
}

func (c *blackholeLibvirtConn) Write(data []byte) (int, error) {
	if !c.stalled && len(data) >= 28 && binary.BigEndian.Uint32(data[4:8]) == 0x20008086 {
		c.requests++
		if c.requests == c.stallAt {
			c.stalled = true
			close(c.reached)
		}
	}
	if c.stalled {
		return len(data), nil
	}
	return c.Conn.Write(data)
}

func TestLibvirtConnectionCancelsStalledRPCs(t *testing.T) {
	for _, stage := range []string{"handshake", "lookup", "state", "destroy", "create", "addresses", "cleanup"} {
		t.Run(stage, func(t *testing.T) {
			mock := libvirttest.New()
			raw, err := mock.Dial()
			if err != nil {
				t.Fatal(err)
			}
			defer raw.Close()
			stallAt := 4
			if stage == "handshake" {
				stallAt = 1
			} else if stage == "lookup" || stage == "cleanup" {
				stallAt = 3
			}
			conn := &blackholeLibvirtConn{Conn: raw, stallAt: stallAt, reached: make(chan struct{})}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			result := make(chan error, 1)
			go func() {
				result <- withLibvirtConnection(ctx, conn, func(lv *libvirt.Libvirt) error {
					if stage == "cleanup" {
						return nil
					}
					domain, err := lv.DomainLookupByName("test")
					if err != nil {
						return err
					}
					switch stage {
					case "state":
						_, _, err = lv.DomainGetState(domain, 0)
					case "destroy":
						err = lv.DomainDestroy(domain)
					case "create":
						err = lv.DomainCreate(domain)
					case "addresses":
						_, err = getDomainIPAddresses(lv, domain)
					}
					return err
				})
			}()
			if stage == "cleanup" {
				select {
				case err := <-result:
					if err != nil {
						t.Fatal(err)
					}
				case <-time.After(3 * time.Second):
					t.Fatal("cleanup waited for a libvirt close RPC")
				}
				return
			}
			select {
			case <-conn.reached:
			case err := <-result:
				t.Fatalf("operation returned before the stalled RPC: %v", err)
			case <-time.After(3 * time.Second):
				t.Fatal("RPC was never reached")
			}
			cancel()
			select {
			case err := <-result:
				if !errors.Is(err, context.Canceled) {
					t.Fatalf("lost cancellation error: %v", err)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("stalled RPC ignored cancellation")
			}
		})
	}
}

func TestLibvirtDomainSkipsCanceledDial(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	err := withLibvirtDomain(ctx, "unused", func(*libvirt.Libvirt, libvirt.Domain) error {
		t.Fatal("operation ran after canceled dial")
		return nil
	})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("lost dial cancellation: %v", err)
	}
}
