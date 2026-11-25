package acl

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
	"time"
)

type testSSHConn struct {
	sshClient
	closes atomic.Int32
	closed chan struct{}
}

func (c *testSSHConn) Close() error {
	if c.closes.Add(1) == 1 && c.closed != nil {
		close(c.closed)
	}
	return nil
}

func TestWithClientInvalidatesFailureWithoutReplay(t *testing.T) {
	failure := errors.New("broken SSH transport")
	for _, operationErr := range []error{nil, failure} {
		name := "success"
		if operationErr != nil {
			name = "failure"
		}
		t.Run(name, func(t *testing.T) {
			conn := &testSSHConn{}
			client := conn
			r := runner{client: client, args: scenarioArgs{OperationTimeout: time.Second}}
			calls := 0
			err := r.withClient(func(context.Context, sshClient) error {
				calls++
				return operationErr
			})
			if !errors.Is(err, operationErr) || calls != 1 {
				t.Fatalf("error = %v, operation calls = %d", err, calls)
			}
			if operationErr != nil {
				if r.client != nil || conn.closes.Load() == 0 {
					t.Fatal("failed connection remained cached or open")
				}
			} else if r.client != client || conn.closes.Load() != 0 {
				t.Fatal("successful connection was not retained")
			}
		})
	}
}

func TestWithClientInvalidatesCanceledOperation(t *testing.T) {
	conn := &testSSHConn{}
	r := runner{client: conn, args: scenarioArgs{OperationTimeout: 20 * time.Millisecond}}
	err := r.withClient(func(ctx context.Context, _ sshClient) error {
		<-ctx.Done()
		return nil
	})
	if !errors.Is(err, context.DeadlineExceeded) || r.client != nil || conn.closes.Load() == 0 {
		t.Fatalf("error = %v, cached client = %v, closes = %d", err, r.client, conn.closes.Load())
	}
}
