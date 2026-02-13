package harpoon

// Generate the Go structs for the Trident protobuf located at ../../../proto/trident
//go:generate ./generate.py

import (
	"context"
	"fmt"
	"net"
	"tridenttools/pkg/harpoon/tridentpbv1"
	"tridenttools/pkg/harpoon/tridentpbv1preview"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

const (
	DefaultTridentSocketPath = "/run/trident/trident.sock"
)

// HarpoonClient is a client for interacting with the Harpoon gRPC service.
type HarpoonClient struct {
	// Stable APIs
	tridentpbv1.VersionServiceClient
	tridentpbv1.StreamingServiceClient
	grpcConn *grpc.ClientConn

	// Preview APIs

	tridentpbv1preview.InstallServiceClient
}

func (c *HarpoonClient) Close() error {
	return c.grpcConn.Close()
}

// NewHarpoonClientFromNetworkConnection creates a new Harpoon gRPC client using
// the provided network connection.
func NewHarpoonClientFromNetworkConnection(conn net.Conn) (*HarpoonClient, error) {
	grpcConn, err := grpc.NewClient(
		"passthrough:target",
		// Not really insecure, we are using a pre-established TLS connection
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(_ context.Context, _ string) (net.Conn, error) {
			return conn, nil
		}),
	)
	if err != nil {
		return nil, fmt.Errorf("failed to create gRPC client: %w", err)
	}

	return &HarpoonClient{
		VersionServiceClient:   tridentpbv1.NewVersionServiceClient(grpcConn),
		StreamingServiceClient: tridentpbv1.NewStreamingServiceClient(grpcConn),
		grpcConn:               grpcConn,
	}, nil
}
