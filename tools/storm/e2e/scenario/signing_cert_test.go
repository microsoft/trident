package scenario

import (
	"os"
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

// os.Stat succeeds for a directory, and so does netlaunch's os.Open preflight,
// so a directory passed as --signing-cert would be accepted here and only fail
// later during firmware-variable setup as an opaque boot failure.
func TestSigningCertRejectsNonRegularFiles(t *testing.T) {
	dir := t.TempDir()

	t.Run("directory is not a certificate", func(t *testing.T) {
		s := &TridentE2EScenario{}
		s.configParams.IsUki = true
		s.args.CertFile = dir
		if _, err := s.signingCertFile(); err == nil {
			t.Error("a directory was accepted as a signing certificate")
		}
	})

	t.Run("non-uki ignores a directory rather than failing", func(t *testing.T) {
		s := &TridentE2EScenario{}
		s.args.CertFile = dir
		got, err := s.signingCertFile()
		if err != nil || got != "" {
			t.Errorf("got (%q, %v), want ignored", got, err)
		}
	})

	t.Run("a real file is accepted", func(t *testing.T) {
		cert := filepath.Join(dir, "ca_cert.pem")
		if err := os.WriteFile(cert, []byte("x"), 0o644); err != nil {
			t.Fatalf("write: %v", err)
		}
		s := &TridentE2EScenario{}
		s.configParams.IsUki = true
		s.args.CertFile = cert
		got, err := s.signingCertFile()
		if err != nil || got != cert {
			t.Errorf("got (%q, %v), want the certificate path", got, err)
		}
	})
}
