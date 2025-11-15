package retry

import (
	"fmt"
	"time"
)

func Retry[T any](timeout, backoff time.Duration, f func(attempt int) (*T, error)) (*T, error) {
	attempt := 0
	startTime := time.Now()
	var err error = nil

	for {
		attempt++
		var result *T
		result, err = f(attempt)
		if err != nil {
			if time.Since(startTime) >= timeout {
				break
			}

			time.Sleep(backoff)
			continue
		}

		return result, nil
	}

	return nil, fmt.Errorf("failed after %d attempts: %w", attempt, err)
}
