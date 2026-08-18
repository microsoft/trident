package proxies

const (
	UpdateRequestAnnotation      = "acl.azure.com/update-request"
	UpdateStatusAnnotation       = "acl.azure.com/update-status"
	UpdateCommitStatusAnnotation = "acl.azure.com/update-commit-status"
	NodeImageVersionLabel        = "kubernetes.azure.com/node-image-version"

	DefaultNodeName   = "trident-node"
	DefaultMarkerFile = "./trident-aks-agent-reboot-signal"
)
