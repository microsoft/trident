package netlaunch

import "testing"

func TestShouldDownloadHostConfiguration(t *testing.T) {
	tests := []struct {
		name     string
		config   RcpConfiguration
		expected bool
	}{
		{
			name:     "CLI install",
			config:   RcpConfiguration{},
			expected: true,
		},
		{
			name: "gRPC install",
			config: RcpConfiguration{
				GrpcMode: true,
			},
			expected: false,
		},
		{
			name: "CLI stream image",
			config: RcpConfiguration{
				UseStreamImage: true,
			},
			expected: false,
		},
		{
			name: "gRPC stream image",
			config: RcpConfiguration{
				GrpcMode:       true,
				UseStreamImage: true,
			},
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			actual := shouldDownloadHostConfiguration(tt.config)
			if actual != tt.expected {
				t.Fatalf("shouldDownloadHostConfiguration() = %t, want %t", actual, tt.expected)
			}
		})
	}
}
