package ioutils

import (
	"io"
	"sync"
)

// NotifyWriter is an io.Writer that notifies when the first successful write
// has occurred.
type NotifyWriter struct {
	W      io.Writer
	once   sync.Once
	Active bool
}

func NewNotifyWriter(w io.Writer) *NotifyWriter {
	return &NotifyWriter{
		W:      w,
		Active: false,
	}
}

func (n *NotifyWriter) Write(p []byte) (int, error) {
	if len(p) == 0 {
		return 0, nil
	}
	nn, err := n.W.Write(p)
	if nn > 0 {
		n.once.Do(func() { n.Active = true })
	}
	return nn, err
}
