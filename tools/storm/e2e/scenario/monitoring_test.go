package scenario

import (
	"context"
	"errors"
	"io"
	"testing"
	"time"
)

type notifyingSerialReader struct {
	io.ReadCloser
	started chan struct{}
}

func (r notifyingSerialReader) Read(p []byte) (int, error) {
	select {
	case r.started <- struct{}{}:
	default:
	}
	return r.ReadCloser.Read(p)
}

func TestReaderLoopCancelsBlockedConsoleRead(t *testing.T) {
	reader, writer := io.Pipe()
	t.Cleanup(func() {
		_ = reader.Close()
		_ = writer.Close()
	})
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	started := make(chan struct{}, 1)
	result := make(chan error, 1)
	go func() {
		result <- readerLoop(ctx, notifyingSerialReader{reader, started}, nil, io.Discard, 30)
	}()

	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("console read did not start")
	}
	cancel()

	select {
	case err := <-result:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("expected canceled console read, got %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("cancellation did not release the silent console")
	}
}
