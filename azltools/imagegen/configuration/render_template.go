package configuration

import (
	_ "embed"
	"fmt"
	"os"
	"path/filepath"
	"text/template"
)

//go:embed template/host-config.yaml.tmpl
var hostConfigTemplate string

// Creates Host Configuration in the specified path, by adding the user input to the template.
func RenderTridentHostConfig(tmplPath string, configData *TridentConfigData, hostconfigPath string) error {
	configDir := filepath.Dir(hostconfigPath)
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("failed to create Host Configuration directory: %w", err)
	}

	// Create scripts directory inside config directory
	scriptsDir := filepath.Join(configDir, "scripts")
	if err := os.MkdirAll(scriptsDir, 0700); err != nil {
		return fmt.Errorf("failed to create scripts directory: %w", err)
	}

	// Write password script
	passwordScriptPath := filepath.Join(scriptsDir, "user-password.sh")
	err := passwordScript(passwordScriptPath, configData)
	if err != nil {
		return fmt.Errorf("failed to write password script: %w", err)
	}
	configData.PasswordScript = passwordScriptPath

	// Render the config file
	var tmpl *template.Template
	if tmplPath == "" {
		tmpl, err = template.New("host-config").Parse(hostConfigTemplate)
	} else {
		tmpl, err = template.ParseFiles(tmplPath)
	}
	if err != nil {
		return fmt.Errorf("failed to parse template: %w", err)
	}
	out, err := os.Create(hostconfigPath)
	if err != nil {
		return fmt.Errorf("failed to create config file: %w", err)
	}
	defer out.Close()
	return tmpl.Execute(out, configData)
}

// Creates the password script at the given path
func passwordScript(passwordScriptPath string, configData *TridentConfigData) (err error) {
	script := fmt.Sprintf("echo '%s:%s' | chpasswd\n", configData.Username, configData.Password)
	dir := filepath.Dir(passwordScriptPath)
	if err = os.MkdirAll(dir, 0700); err != nil {
		return
	}
	if err = os.WriteFile(passwordScriptPath, []byte(script), 0700); err != nil {
		return
	}
	return
}
