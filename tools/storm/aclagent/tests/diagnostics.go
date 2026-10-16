package tests

import "strings"

// Select message lines, not raw XML continuations or unrelated network logs.
func nebraskaDiagnosticLines(journal string) []string {
	var lines []string
	for _, line := range strings.Split(journal, "\n") {
		if strings.Contains(line, "Nebraska ") || strings.Contains(line, "Resolved COSI metadata SHA-384:") {
			lines = append(lines, line)
		}
	}
	return lines
}
