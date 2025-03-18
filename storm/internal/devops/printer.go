package devops

import (
	"fmt"
)

func LogError(msg string, a ...any) {
	fmt.Printf("##vso[task.logissue type=error]%s\n", fmt.Sprintf(msg, a...))
}

func LogWarning(msg string, a ...any) {
	fmt.Printf("##vso[task.logissue type=warning]%s\n", fmt.Sprintf(msg, a...))
}
