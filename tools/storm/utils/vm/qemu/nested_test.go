package qemu

import (
	"errors"
	"strings"
	"testing"
)

func TestNestedVirtualizationCPU(t *testing.T) {
	tests := []struct {
		name, info, module, enabled, cpu string
		readErr                          error
	}{
		{name: "Intel", info: "processor : 0\nflags : fpu vmx sse\n", module: "kvm_intel", enabled: "Y\n", cpu: "host-passthrough,+vmx"},
		{name: "AMD", info: "flags : fpu svm sse\n", module: "kvm_amd", enabled: "1\n", cpu: "host-passthrough,+svm"},
		{name: "virtualization hidden", info: "flags : fpu sse\n"},
		{name: "not a CPU flag", info: "model name : vmx\nflags : fpu sse\n"},
		{name: "nested disabled", info: "flags : vmx", module: "kvm_intel", enabled: "N\n"},
		{name: "KVM module unavailable", info: "flags : svm", module: "kvm_amd", readErr: errors.New("module missing")},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cpu, err := nestedVirtualizationCPU([]byte(tt.info), func(path string) ([]byte, error) {
				if path != "/sys/module/"+tt.module+"/parameters/nested" {
					t.Fatalf("unexpected nested setting: %s", path)
				}
				return []byte(tt.enabled), tt.readErr
			})
			if cpu != tt.cpu {
				t.Fatalf("CPU=%q want %q", cpu, tt.cpu)
			}
			if tt.cpu == "" && (err == nil || !strings.Contains(err.Error(), "nested virtualization")) {
				t.Fatalf("missing actionable preflight failure: %v", err)
			}
			if tt.cpu != "" && err != nil {
				t.Fatal(err)
			}
		})
	}
}
