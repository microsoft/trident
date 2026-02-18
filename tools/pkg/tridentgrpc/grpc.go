package tridentgrpc

// Generate the Go structs for the Trident protobuf located at ../../../proto/trident
//go:generate ./generate.py

import (
	"context"
	"fmt"
	"net"
	"tridenttools/pkg/tridentgrpc/tridentpbv1"
	"tridenttools/pkg/tridentgrpc/tridentpbv1preview"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

const (
	DefaultTridentSocketPath = "/run/trident/trident.sock"
)

// TridentClient is a client for interacting with the Trident gRPC service.
type TridentClient struct {
	// Stable APIs
	tridentpbv1.VersionServiceClient
	tridentpbv1.StreamingServiceClient
	grpcConn *grpc.ClientConn

	// Preview APIs

	tridentpbv1preview.InstallServiceClient
}

func (c *TridentClient) Close() error {
	return c.grpcConn.Close()
}

// NewTridentClientFromNetworkConnection creates a new Trident gRPC client using
// the provided network connection.
func NewTridentClientFromNetworkConnection(conn net.Conn) (*TridentClient, error) {
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

	return &TridentClient{
		VersionServiceClient:   tridentpbv1.NewVersionServiceClient(grpcConn),
		StreamingServiceClient: tridentpbv1.NewStreamingServiceClient(grpcConn),
		grpcConn:               grpcConn,
	}, nil
}
