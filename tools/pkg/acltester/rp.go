package acltester

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
		if err := c.patchNodeLabels(ctx, step.Patch.Labels()); err != nil {
			return nil, err
		}
		return &StepReport{Index: index, Kind: "patch", Passed: true, Message: "patched fake Node labels"}, nil
	case step.Expect != nil:
		return c.expectState(ctx, index, step.Expect)
	case step.AssertFailureReason != "":
		node, err := c.getNode(ctx)
		if err != nil {
			return nil, err
		}
		actual := node.Labels[FailureReasonLabel]
		passed := actual == step.AssertFailureReason
		message := "failure reason matched"
		if !passed {
			message = "failure reason mismatch"
		}
		return &StepReport{
			Index:    index,
			Kind:     "assert-failure-reason",
			Passed:   passed,
			Message:  message,
			Expected: step.AssertFailureReason,
			Actual:   actual,
		}, nil
	default:
		return nil, fmt.Errorf("step %d had no recognized action", index)
	}
}

func (c *RPClient) expectState(ctx context.Context, index int, step *ExpectStep) (*StepReport, error) {
	deadline := time.Now().Add(step.Timeout)
	pollInterval := 500 * time.Millisecond
	var lastObserved map[string]string
	matched := false

	for time.Now().Before(deadline) {
		node, err := c.getNode(ctx)
		if err != nil {
			return nil, err
		}
		lastObserved = map[string]string{
			"state":               node.Labels[StateLabel],
			"observed-request-id": node.Labels[ObservedRequestIDLabel],
			"failure-reason":      node.Labels[FailureReasonLabel],
		}
		if node.Labels[StateLabel] == step.State {
			if step.ObservedRequestID == "" || node.Labels[ObservedRequestIDLabel] == step.ObservedRequestID {
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
	message := "observed expected state"
	if step.ExpectTimeout {
		passed = !matched
		message = "timed out as expected"
	}
	if !passed && !step.ExpectTimeout {
		message = "state expectation failed"
	}
	return &StepReport{
		Index:   index,
		Kind:    "expect",
		Passed:  passed,
		Message: message,
		Expected: map[string]any{
			"state":               step.State,
			"observed-request-id": step.ObservedRequestID,
			"timeout":             step.Timeout.String(),
			"expect-timeout":      step.ExpectTimeout,
		},
		Actual: lastObserved,
	}, nil
}

func (c *RPClient) patchNodeLabels(ctx context.Context, labels map[string]string) error {
	body, err := json.Marshal(map[string]any{
		"metadata": map[string]any{
			"labels": labels,
		},
	})
	if err != nil {
		return err
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodPatch, c.nodeURL(), bytes.NewReader(body))
	if err != nil {
		return err
	}
	request.Header.Set("Content-Type", "application/merge-patch+json")
	response, err := c.client().Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode >= 300 {
		return fmt.Errorf("fake apiserver patch failed with %s", response.Status)
	}
	return nil
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
