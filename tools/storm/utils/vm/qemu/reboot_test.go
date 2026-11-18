package qemu

import (
	"context"
	"errors"
	"slices"
	"testing"

	"github.com/digitalocean/go-libvirt"
)

type fakeRestarter struct {
	calls []string
	poll  func() (int32, int32, error)
}

func (f *fakeRestarter) DomainShutdown(libvirt.Domain) error {
	f.calls = append(f.calls, "shutdown")
	return nil
}
func (f *fakeRestarter) DomainGetState(libvirt.Domain, uint32) (int32, int32, error) {
	f.calls = append(f.calls, "state")
	return f.poll()
}
func (f *fakeRestarter) DomainCreate(libvirt.Domain) error {
	f.calls = append(f.calls, "start")
	return nil
}

func TestRestartWaitsForShutoff(t *testing.T) {
	f := &fakeRestarter{poll: func() (int32, int32, error) { return int32(libvirt.DomainShutoff), 0, nil }}
	if err := restartDomain(context.Background(), f, libvirt.Domain{}); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(f.calls, []string{"shutdown", "state", "start"}) {
		t.Fatalf("calls: %v", f.calls)
	}
}

func TestRestartNeverStartsAfterCancellationOrPollError(t *testing.T) {
	for _, cancelled := range []bool{false, true} {
		ctx, cancel := context.WithCancel(context.Background())
		pollErr := errors.New("libvirt failed")
		expected := error(pollErr)
		if cancelled {
			expected = context.Canceled
		}
		f := &fakeRestarter{poll: func() (int32, int32, error) {
			if cancelled {
				cancel()
				return int32(libvirt.DomainRunning), 0, nil
			}
			return 0, 0, pollErr
		}}
		err := restartDomain(ctx, f, libvirt.Domain{})
		cancel()
		if !errors.Is(err, expected) || slices.Contains(f.calls, "start") {
			t.Fatalf("calls=%v err=%v", f.calls, err)
		}
	}
}
