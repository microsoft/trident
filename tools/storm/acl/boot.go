package acl

import (
	"context"
	"errors"
	"fmt"
)

type bootConnection struct {
	ip, id string
	client sshClient
}

// waitForBootCandidates probes leases concurrently so a stale address cannot starve a live one.
func waitForBootCandidates(ctx context.Context, ips []string, probe func(context.Context, string) (sshClient, string, error)) (*bootConnection, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	type result struct {
		bootConnection
		err error
	}
	results := make(chan result)
	seen := make(map[string]bool)
	for _, ip := range ips {
		if ip == "" || seen[ip] {
			continue
		}
		seen[ip] = true
		go func() {
			client, id, err := probe(ctx, ip)
			got := result{bootConnection{ip: ip, id: id, client: client}, err}
			select {
			case results <- got:
			case <-ctx.Done():
				if client != nil {
					_ = client.Close()
				}
			}
		}()
	}
	if len(seen) == 0 {
		return nil, errors.New("no guest addresses")
	}
	var resultErr error
	for range len(seen) {
		select {
		case <-ctx.Done():
			return nil, errors.Join(resultErr, ctx.Err())
		case got := <-results:
			if got.err == nil && ctx.Err() == nil {
				return &got.bootConnection, nil
			}
			if got.client != nil {
				_ = got.client.Close()
			}
			resultErr = errors.Join(resultErr, fmt.Errorf("guest address %s: %w", got.ip, errors.Join(got.err, ctx.Err())))
		}
	}
	return nil, errors.Join(resultErr, ctx.Err())
}
