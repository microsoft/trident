package proxies

const (
	UpdateRequestAnnotation = "acl.azure.com/update-request"
	UpdateStatusAnnotation  = "acl.azure.com/update-status"
	NodeImageVersionLabel   = "kubernetes.azure.com/node-image-version"

	DefaultNodeName   = "trident-node"
	DefaultMarkerFile = "./trident-acl-agent-reboot-signal"
)
