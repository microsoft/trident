package proxies

const (
	UpdateRequestAnnotation      = "acl.azure.com/update-request"
	UpdateStatusAnnotation       = "acl.azure.com/update-status"
	UpdateCommitStatusAnnotation = "acl.azure.com/update-commit-status"
	NodeImageVersionLabel        = "kubernetes.azure.com/node-image-version"

	DefaultNodeName   = "trident-node"
	DefaultMarkerFile = "./trident-acl-agent-reboot-signal"

	// PostgresImage is the ephemeral Postgres image NebraskaProxy runs to
	// back the real Nebraska server it links in-process. Pulled from the
	// maritimusdev ACR (mirroring the same registry push-to-acr.yml uses for
	// the Polar SC, see maritimus-dev-acr-write-umi) rather than unqualified
	// from Docker Hub, since some pipeline pools (e.g. OneBranch-governed
	// ones) restrict egress to an allow-list that does not include Docker
	// Hub.
	PostgresImage = "maritimusdev.azurecr.io/postgres:16-alpine"
)
