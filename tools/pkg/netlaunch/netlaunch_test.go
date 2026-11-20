package netlaunch

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestISOHandlerLifetime(t *testing.T) {
	iso := []byte("iso")
	logRequest := func(string) {}
	handler := isoHandler(iso, logRequest)

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
	}
}
