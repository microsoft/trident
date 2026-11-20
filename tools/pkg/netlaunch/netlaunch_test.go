package netlaunch

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestISOHandlerLifetime(t *testing.T) {
	iso := []byte("iso")
	logRequest := func(string) {}

	t.Run("one shot", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		request := httptest.NewRequest(http.MethodGet, "/provision.iso", nil)
		response := httptest.NewRecorder()

		isoHandler(iso, logRequest, cancel).ServeHTTP(response, request)

		if response.Code != http.StatusOK {
			t.Fatalf("expected status %d, got %d", http.StatusOK, response.Code)
		}
		if response.Body.String() != string(iso) {
			t.Fatalf("expected ISO contents %q, got %q", iso, response.Body.String())
		}
		if ctx.Err() != context.Canceled {
			t.Fatalf("expected context cancellation, got %v", ctx.Err())
		}
	})

	t.Run("persistent", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		handler := isoHandler(iso, logRequest, nil)

		for range 2 {
			request := httptest.NewRequest(http.MethodGet, "/provision.iso", nil)
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, request)

			if response.Code != http.StatusOK {
				t.Fatalf("expected status %d, got %d", http.StatusOK, response.Code)
			}
			if response.Body.String() != string(iso) {
				t.Fatalf("expected ISO contents %q, got %q", iso, response.Body.String())
			}
			if ctx.Err() != nil {
				t.Fatalf("expected listener context to remain active, got %v", ctx.Err())
			}
		}
	})
}
