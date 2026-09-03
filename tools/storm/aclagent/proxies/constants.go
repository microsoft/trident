package proxies

const (
	UpdateRequestAnnotation      = "acl.azure.com/update-request"
	UpdateStatusAnnotation       = "acl.azure.com/update-status"
	UpdateCommitStatusAnnotation = "acl.azure.com/update-commit-status"
	NodeImageVersionLabel        = "kubernetes.azure.com/node-image-version"

	DefaultNodeName   = "trident-node"
	DefaultMarkerFile = "./trident-acl-agent-reboot-signal"

	// DefaultPostgresImage is the ephemeral Postgres image NebraskaProxy runs
	// to back the real Nebraska server it links in-process, used whenever a
	// caller (or the --postgres-image CLI flag, see
	// utils/config.TestConfig.PostgresImage) does not set
	// NebraskaProxy.PostgresImage explicitly. This is the public Docker Hub
	// image deliberately, not an internal registry, so the harness (and its
	// unit tests) work unmodified for anyone with plain internet access - no
	// Microsoft-internal credentials or network access required. Trident's
	// own pipelines run on network-restricted pools and override this to an
	// internal ACR mirror via that same flag.
	DefaultPostgresImage = "postgres:16-alpine"
)