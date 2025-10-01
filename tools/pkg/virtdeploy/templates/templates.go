package templates

import (
	"fmt"
	"html/template"
	"strings"
)

func renderTemplate(name string, templateStr string, data any) (string, error) {
	template, err := template.New(name).Parse(templateStr)

	if err != nil {
		return "", fmt.Errorf("parse %s XML template: %w", name, err)
	}

	var buf strings.Builder
	err = template.Execute(&buf, data)
	if err != nil {
		return "", fmt.Errorf("execute %s XML template: %w", name, err)
	}
	return buf.String(), nil
}
