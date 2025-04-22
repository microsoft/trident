package runner

import (
	"fmt"
	"storm/internal/stormerror"
	"testing"
)

func TestRunCatchPanic(t *testing.T) {
	t.Run("no panic", func(t *testing.T) {
		err := runCatchPanic(func() error { return nil })
		if err != nil {
			t.Errorf("expected no error, got %v", err)
		}
	})

	t.Run("error", func(t *testing.T) {
		err := runCatchPanic(func() error { return fmt.Errorf("test error") })
		if err == nil {
			t.Errorf("expected an error, got nil")
		}

		if _, ok := err.(*stormerror.PanicError); ok {
			t.Errorf("expected non-panic error, got panic error")
		}

		if err.Error() != "test error" {
			t.Errorf("expected test error, got %v", err)
		}
	})

	t.Run("panic", func(t *testing.T) {
		err := runCatchPanic(func() error {
			panic("test panic")
		})
		if err == nil {
			t.Errorf("expected an error, got nil")
		}

		pe, ok := err.(stormerror.PanicError)
		if !ok {
			t.Errorf("expected panic error, got non-panic error")
		}

		if pe.Error() != "panic occurred: test panic" {
			t.Errorf("expected panic error, got %v", pe)
		}
	})
}
