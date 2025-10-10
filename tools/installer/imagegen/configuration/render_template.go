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
func RenderTridentHostConfig(configPath string, configData *TridentConfigData) error {
	configDir := filepath.Dir(configPath)
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("failed to create Host Configuration directory: %w", err)
	}

	// Create scripts directory inside Host configuration directory
	scriptsDir := filepath.Join(configDir, "scripts")
	if err := os.MkdirAll(scriptsDir, 0700); err != nil {
		return fmt.Errorf("failed to create scripts directory: %w", err)
	}

	// Generate and write the user password script to set the user password
	passwordScriptPath := filepath.Join(scriptsDir, "user-password.sh")
	err := passwordScript(passwordScriptPath, configData)
	if err != nil {
		return fmt.Errorf("failed to write password script: %w", err)
	}
	configData.PasswordScript = passwordScriptPath

	var templateContent = hostConfigTemplate

	// Render the config file
	tmpl, err := template.New("host-config").Parse(templateContent)
	if err != nil {
		return fmt.Errorf("failed to parse template: %w", err)
	}
	out, err := os.Create(configPath)
	if err != nil {
		return fmt.Errorf("failed to create config file: %w", err)
	}
	defer out.Close()
	return tmpl.Execute(out, configData)
}

// passwordScript generates and writes a shell script that sets the user password.
// The script uses chpasswd with the -e flag to accept pre-hashed passwords (password is already hashed).
func passwordScript(passwordScriptPath string, configData *TridentConfigData) (err error) {
	if configData.HashedPassword == "" {
		return fmt.Errorf("hashed password is required but not set")
	}

	script := fmt.Sprintf("echo '%s:%s' | chpasswd -e\n", configData.Username, configData.HashedPassword)
	dir := filepath.Dir(passwordScriptPath)
	if err = os.MkdirAll(dir, 0700); err != nil {
		return
	}
	if err = os.WriteFile(passwordScriptPath, []byte(script), 0700); err != nil {
		return
	}
	return
}
