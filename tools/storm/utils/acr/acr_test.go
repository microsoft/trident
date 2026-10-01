package acr

import (
	"errors"
	"strings"
	"testing"

	"oras.land/oras-go/v2/errdef"
)

func TestNewClientRequiresCredentials(t *testing.T) {
	if _, err := NewClient("", "token"); err == nil {
		t.Error("an empty ACR name must be rejected")
	}
	if _, err := NewClient("acr", ""); err == nil {
		t.Error("an empty refresh token must be rejected")
	}
	if _, err := NewClient("acr", "token"); err != nil {
		t.Errorf("valid credentials rejected: %v", err)
	}
}

// Trident consumes image.url in this exact form.
func TestReferenceRendersAnOciUrl(t *testing.T) {
	c, err := NewClient("maritimusdev", "token")
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	const want = "oci://maritimusdev.azurecr.io/cosi-storm-host:v1.misc.virtualMachine.1"
	if got := c.Reference("cosi-storm-host", "v1.misc.virtualMachine.1"); got != want {
		t.Errorf("Reference = %q, want %q", got, want)
	}
}

// Cleanup treats "not found" as success. Misclassifying an auth or transport
// failure as absence would silently skip the delete and leak images, so only a
// genuine not-found may match.
func TestIsNotFoundOnlyMatchesAbsence(t *testing.T) {
	if !isNotFound(errdef.ErrNotFound) {
		t.Error("errdef.ErrNotFound must count as absent")
	}
	if !isNotFound(errors.New("MANIFEST_UNKNOWN: manifest tagged by \"x\" is not found")) {
		t.Error("a registry MANIFEST_UNKNOWN must count as absent")
	}
	for _, e := range []string{
		"UNAUTHORIZED: authentication required",
		"connection refused",
		"TOOMANYREQUESTS: retry later",
	} {
		if isNotFound(errors.New(e)) {
			t.Errorf("%q must not be read as absence", strings.SplitN(e, ":", 2)[0])
		}
	}
}
