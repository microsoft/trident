package harpoon

// Generate the Go structs for the Harpoon protobuf located at ../../../proto/harpoon/v1/harpoon.proto
//go:generate protoc -I ../../../proto/harpoon/v1 --go_out=harpoonpbv1 --go_opt=paths=source_relative --go_opt=Mharpoon.proto=tridenttools/pkg/harpoon/harpoonpbv1 --go-grpc_out=harpoonpbv1 --go-grpc_opt=paths=source_relative --go-grpc_opt=Mharpoon.proto=tridenttools/pkg/harpoon/harpoonpbv1 harpoon.proto

import (
	"context"
	"fmt"
	"net"
	"tridenttools/pkg/harpoon/harpoonpbv1"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

const (
	DefaultTridentSocketPath = "/run/trident.sock"
)

// HarpoonClient is a client for interacting with the Harpoon gRPC service.
type HarpoonClient struct {
	harpoonpbv1.TridentServiceClient
	grpcConn *grpc.ClientConn
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
		TridentServiceClient: harpoonpbv1.NewTridentServiceClient(grpcConn),
		grpcConn:             grpcConn,
	}, nil
}
