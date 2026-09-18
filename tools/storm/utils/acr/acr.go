// Package acr hosts the storm suite's own Azure Container Registry client.
//
// This is deliberately separate from tools/storm/scripts/acr, which belongs to
// the legacy pytest suite. That suite is still the production gate, so its
// scripts are left untouched: sharing them meant a change made for storm could
// break the pipeline that currently guards main, which is what happened when
// their command-line contract was altered.
//
// Transfers happen in-process with oras-go rather than by shelling out to the
// oras binary. The library is already a dependency of this module (see
// storm/helpers/ab_update.go, which pulls artifacts with it), so invoking the
// binary meant downloading a tool at test time to do something the test binary
// could already do.
package acr

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"

	ocispec "github.com/opencontainers/image-spec/specs-go/v1"
	"oras.land/oras-go/v2"
	"oras.land/oras-go/v2/content/file"
	"oras.land/oras-go/v2/errdef"
	"oras.land/oras-go/v2/registry/remote"
	"oras.land/oras-go/v2/registry/remote/auth"
	"oras.land/oras-go/v2/registry/remote/retry"
)

// artifactMediaType is the media type for the files this suite hosts in ACR.
// COSI images and system extension images alike are opaque blobs.
const artifactMediaType = "application/vnd.microsoft.trident.testartifact"

// acrRefreshTokenUser is the username ACR expects when authenticating with a
// refresh token; the token itself carries the identity.
const acrRefreshTokenUser = "00000000-0000-0000-0000-000000000000"

// Client pushes and deletes artifacts in one Azure Container Registry.
type Client struct {
	registry string
	cred     auth.CredentialFunc
}

// NewClient returns a client for <acrName>.azurecr.io authenticated with an
// ACR refresh token.
func NewClient(acrName, refreshToken string) (*Client, error) {
	if acrName == "" {
		return nil, errors.New("ACR name is required")
	}
	if refreshToken == "" {
		return nil, errors.New("ACR refresh token is required")
	}
	registry := acrName + ".azurecr.io"
	return &Client{
		registry: registry,
		cred: auth.StaticCredential(registry, auth.Credential{
			Username: acrRefreshTokenUser,
			Password: refreshToken,
		}),
	}, nil
}

// repository returns an authenticated client for one repository.
func (c *Client) repository(name string) (*remote.Repository, error) {
	repo, err := remote.NewRepository(fmt.Sprintf("%s/%s", c.registry, name))
	if err != nil {
		return nil, fmt.Errorf("failed to address repository %q: %w", name, err)
	}
	repo.Client = &auth.Client{
		Client:     retry.DefaultClient,
		Cache:      auth.NewCache(),
		Credential: c.cred,
	}
	return repo, nil
}

// Push uploads filePath to <repository>:<tag> and returns the reference it was
// published under.
func (c *Client) Push(ctx context.Context, repository, tag, filePath string) (string, error) {
	repo, err := c.repository(repository)
	if err != nil {
		return "", err
	}

	// The descriptor is named after the file's basename, which is how the
	// artifact is identified when pulled back.
	store, err := file.New(filepath.Dir(filePath))
	if err != nil {
		return "", fmt.Errorf("failed to open a file store for %q: %w", filePath, err)
	}
	defer store.Close()

	name := filepath.Base(filePath)
	desc, err := store.Add(ctx, name, artifactMediaType, filePath)
	if err != nil {
		return "", fmt.Errorf("failed to add %q to the file store: %w", filePath, err)
	}

	manifest, err := oras.PackManifest(ctx, store, oras.PackManifestVersion1_1,
		artifactMediaType, oras.PackManifestOptions{Layers: []ocispec.Descriptor{desc}})
	if err != nil {
		return "", fmt.Errorf("failed to pack a manifest for %q: %w", name, err)
	}
	if err := store.Tag(ctx, manifest, tag); err != nil {
		return "", fmt.Errorf("failed to tag %q locally: %w", name, err)
	}

	if _, err := oras.Copy(ctx, store, tag, repo, tag, oras.DefaultCopyOptions); err != nil {
		return "", fmt.Errorf("failed to push %q to %s/%s:%s: %w", name, c.registry, repository, tag, err)
	}
	return c.Reference(repository, tag), nil
}

// Exists reports whether a tag is present in the repository.
func (c *Client) Exists(ctx context.Context, repository, tag string) (bool, error) {
	repo, err := c.repository(repository)
	if err != nil {
		return false, err
	}
	if _, err := repo.Resolve(ctx, tag); err != nil {
		if isNotFound(err) {
			return false, nil
		}
		return false, fmt.Errorf("failed to resolve %s:%s: %w", repository, tag, err)
	}
	return true, nil
}

// Delete removes a tag if it is present. An absent tag is not an error:
// cleanup runs unconditionally after a run, including runs that failed before
// pushing anything.
func (c *Client) Delete(ctx context.Context, repository, tag string) error {
	repo, err := c.repository(repository)
	if err != nil {
		return err
	}
	desc, err := repo.Resolve(ctx, tag)
	if err != nil {
		if isNotFound(err) {
			return nil
		}
		return fmt.Errorf("failed to resolve %s:%s: %w", repository, tag, err)
	}
	if err := repo.Delete(ctx, desc); err != nil {
		return fmt.Errorf("failed to delete %s:%s: %w", repository, tag, err)
	}
	return nil
}

// Reference renders the OCI URL Trident consumes as image.url.
func (c *Client) Reference(repository, tag string) string {
	return fmt.Sprintf("oci://%s/%s:%s", c.registry, repository, tag)
}

// isNotFound distinguishes "this tag is not in the registry" from a transport
// or authentication failure, which must not be read as absence: treating an
// auth error as "already gone" would silently skip cleanup and leak images.
func isNotFound(err error) bool {
	return errors.Is(err, errdef.ErrNotFound) || strings.Contains(err.Error(), "MANIFEST_UNKNOWN")
}
