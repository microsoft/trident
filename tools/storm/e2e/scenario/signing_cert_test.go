package scenario

import (
	"path/filepath"
	"testing"
)

// netlaunch only enrolls the certificate when the path is non-empty, so for a
// UKI configuration a missing one must fail here rather than booting a VM that
// cannot verify its kernel and timing out with no explanation.
func TestSigningCertFileRequiredOnlyForUki(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "absent.pem")

	t.Run("uki with no flag fails", func(t *testing.T) {
		s := &TridentE2EScenario{}
		s.configParams.IsUki = true
		if _, err := s.signingCertFile(); err == nil {
			t.Error("expected an error for a UKI config with no --signing-cert")
		}
	})

	t.Run("uki with unreadable file fails", func(t *testing.T) {
		s := &TridentE2EScenario{}
		s.configParams.IsUki = true
		s.args.CertFile = missing
		if _, err := s.signingCertFile(); err == nil {
			t.Error("expected an error for a UKI config whose certificate is absent")
		}
	})

	t.Run("non-uki stays lenient", func(t *testing.T) {
		s := &TridentE2EScenario{}
		s.args.CertFile = missing
		got, err := s.signingCertFile()
		if err != nil || got != "" {
			t.Errorf("non-UKI should continue without a certificate, got (%q, %v)", got, err)
		}
	})
}
