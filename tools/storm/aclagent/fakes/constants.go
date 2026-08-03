package fakes

import (
	"fmt"
	"strings"
)

const (
	RequestLabel            = "kubernetes.azure.com/trident-abupdate-request"
	RequestIDLabel          = "kubernetes.azure.com/trident-abupdate-request-id"
	TargetVersionLabel      = "kubernetes.azure.com/trident-abupdate-target-os-image-version"
	StateLabel              = "kubernetes.azure.com/trident-abupdate-state"
	ObservedRequestIDLabel  = "kubernetes.azure.com/trident-abupdate-observed-request-id"
	FailureReasonLabel      = "kubernetes.azure.com/trident-abupdate-failure-reason"
	NodeImageVersionLabel   = "kubernetes.azure.com/node-image-version"
	FailureDetailAnnotation = "kubernetes.azure.com/trident-abupdate-failure-detail"

	DefaultNodeName   = "trident-node"
	DefaultMarkerFile = "./trident-acl-agent-tester-reboot-signal"
)

func ParseKeyValueList(raw string) (map[string]string, error) {
	result := map[string]string{}
	if strings.TrimSpace(raw) == "" {
		return result, nil
	}
	for _, pair := range strings.Split(raw, ",") {
		pair = strings.TrimSpace(pair)
		if pair == "" {
			continue
		}
		key, value, ok := strings.Cut(pair, "=")
		if !ok || strings.TrimSpace(key) == "" {
			return nil, fmt.Errorf("invalid key=value pair %q", pair)
		}
		result[strings.TrimSpace(key)] = strings.TrimSpace(value)
	}
	return result, nil
}
