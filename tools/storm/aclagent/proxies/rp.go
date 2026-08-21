package proxies

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"

	corev1 "k8s.io/api/core/v1"
)

type RPClient struct {
	HTTPClient   *http.Client
	APIServerURL string
	NodeName     string
}

type updateRequest struct {
	SchemaVersion string `json:"schemaVersion"`
	NodeUpdateID  string `json:"nodeUpdateId"`
	OperationID   string `json:"operationId"`
	Operation     string `json:"operation"`
	TargetVersion string `json:"targetVersion,omitempty"`
	Server        string `json:"server,omitempty"`
	AppId         string `json:"appId,omitempty"`
	Track         string `json:"track,omitempty"`
}

type updateStatus struct {
	SchemaVersion string `json:"schemaVersion"`
	NodeUpdateID  string `json:"nodeUpdateId"`
	OperationID   string `json:"operationId"`
	Operation     string `json:"operation"`
	Code          string `json:"code"`
}

func (c *RPClient) RunScenario(ctx context.Context, scenario *Scenario) (*ScenarioReport, error) {
	report := &ScenarioReport{Steps: make([]StepReport, 0, len(scenario.Steps)), Passed: true}
	for index, step := range scenario.Steps {
		started := time.Now()
		stepReport, err := c.runStep(ctx, index, step)
		if err != nil {
			return nil, err
		}
		stepReport.ElapsedMS = time.Since(started).Milliseconds()
		report.Steps = append(report.Steps, *stepReport)
		report.Passed = report.Passed && stepReport.Passed
	}
	return report, nil
}

func (c *RPClient) runStep(ctx context.Context, index int, step ScenarioStep) (*StepReport, error) {
	switch {
	case step.Patch != nil:
		if err := c.patchNodeRequest(ctx, step.Patch); err != nil {
			return nil, err
		}
		return &StepReport{Index: index, Kind: "patch", Passed: true, Message: "patched fake Node request annotation"}, nil
	case step.Expect != nil:
		return c.expectStatus(ctx, index, step.Expect)
	default:
		return nil, fmt.Errorf("step %d had no recognized action", index)
	}
}

func (c *RPClient) expectStatus(ctx context.Context, index int, step *ExpectStep) (*StepReport, error) {
	deadline := time.Now().Add(step.Timeout)
	pollInterval := 500 * time.Millisecond
	annotationKey := UpdateStatusAnnotation
	if step.Operation == "commit" {
		annotationKey = UpdateCommitStatusAnnotation
	}
	var lastObserved map[string]string
	matched := false
	for time.Now().Before(deadline) {
		node, err := c.getNode(ctx)
		if err != nil {
			return nil, err
		}
		status, err := decodeStatus(node, annotationKey)
		if err != nil {
			return nil, fmt.Errorf("failed to decode %s annotation: %w", annotationKey, err)
		}
		if status != nil {
			lastObserved = map[string]string{"operation-id": status.OperationID, "operation": status.Operation, "code": status.Code}
			if status.Code == step.Code && (step.OperationID == "" || status.OperationID == step.OperationID) && (step.Operation == "" || status.Operation == step.Operation) {
				matched = true
				break
			}
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(pollInterval):
		}
	}
	passed := matched
	message := "observed expected status"
	if step.ExpectTimeout {
		passed = !matched
		if passed {
			message = "timed out as expected"
		} else {
			message = "expected no matching status before timeout, but status matched"
		}
	} else if !passed {
		message = "status expectation failed"
	}
	return &StepReport{Index: index, Kind: "expect", Passed: passed, Message: message, Expected: map[string]any{"operation-id": step.OperationID, "operation": step.Operation, "code": step.Code, "timeout": step.Timeout.String()}, Actual: lastObserved}, nil
}

func (c *RPClient) patchNodeRequest(ctx context.Context, step *PatchStep) error {
	request := updateRequest{SchemaVersion: "1.0", NodeUpdateID: step.NodeUpdateID, OperationID: step.OperationID, Operation: step.Operation, TargetVersion: step.TargetOSImageVersion, Server: step.Server, AppId: step.AppId, Track: step.Track}
	raw, err := json.Marshal(request)
	if err != nil {
		return err
	}
	body, err := json.Marshal(map[string]any{"metadata": map[string]any{"annotations": map[string]string{UpdateRequestAnnotation: string(raw)}}})
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPatch, c.nodeURL(), bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/merge-patch+json")
	resp, err := c.client().Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return fmt.Errorf("fake apiserver patch failed with %s", resp.Status)
	}
	return nil
}

func decodeStatus(node *corev1.Node, annotationKey string) (*updateStatus, error) {
	raw := node.Annotations[annotationKey]
	if raw == "" {
		return nil, nil
	}
	var status updateStatus
	if err := json.Unmarshal([]byte(raw), &status); err != nil {
		return nil, err
	}
	return &status, nil
}

func (c *RPClient) getNode(ctx context.Context) (*corev1.Node, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, c.nodeURL(), nil)
	if err != nil {
		return nil, err
	}
	response, err := c.client().Do(request)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	if response.StatusCode >= 300 {
		return nil, fmt.Errorf("fake apiserver get failed with %s", response.Status)
	}
	var node corev1.Node
	if err := json.NewDecoder(response.Body).Decode(&node); err != nil {
		return nil, fmt.Errorf("failed to decode fake Node response: %w", err)
	}
	return &node, nil
}

func (c *RPClient) nodeURL() string {
	return strings.TrimRight(c.APIServerURL, "/") + "/api/v1/nodes/" + c.NodeName
}

func (c *RPClient) client() *http.Client {
	if c.HTTPClient != nil {
		return c.HTTPClient
	}
	return http.DefaultClient
}
