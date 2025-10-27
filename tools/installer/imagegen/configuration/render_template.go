package configuration

import (
	_ "embed"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"text/template"

	"installer/internal/file"
)

//go:embed template/host-config.yaml.tmpl
var hostConfigTemplate string

// Use default embedded template
func RenderTridentHostConfiguration(configPath string, configData *TridentConfigData) error {
	return RenderHostConfigurationWithTemplate(configPath, configData, hostConfigTemplate)
}

// Creates Host Configuration from the given template for unattended installation
func RenderHostConfigurationUnattended(templatePath string, devicePath string) (configPath string, err error) {
	// Check if template file exists
	exists, err := file.PathExists(templatePath)
	if err != nil {
		return
	}
	if !exists {
		return "", fmt.Errorf("template file does not exist: %s", templatePath)
	}

	templateContent, err := os.ReadFile(templatePath)
	if err != nil {
		return "", fmt.Errorf("failed to read given Host Configuration template file %s: %w", templatePath, err)
	}

	configData := NewTridentConfigData()
	configData.DiskPath = devicePath

	configDir := filepath.Dir(templatePath)
	templateName := filepath.Base(templatePath)
	// Remove ".tmpl" for Host Configuration file name
	templateName = strings.TrimSuffix(templateName, ".tmpl")
	configPath = filepath.Join(configDir, templateName)

	err = RenderHostConfigurationWithTemplate(configPath, configData, string(templateContent))
	if err != nil {
		return "", err
	}

	return configPath, nil
}

// Creates Host Configuration in the specified path, by adding the user input to the template
func RenderHostConfigurationWithTemplate(configPath string, configData *TridentConfigData, templateContent string) error {
	configDir := filepath.Dir(configPath)
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("failed to create Host Configuration directory: %w", err)
	}

	if configData.HashedPassword != "" {
		// Create scripts directory inside Host configuration directory
		scriptsDir := filepath.Join(configDir, "scripts")
		if err := os.MkdirAll(scriptsDir, 0700); err != nil {
			return fmt.Errorf("failed to create scripts directory: %w", err)
		}

		// Generate script to set the user password
		passwordScriptPath := filepath.Join(scriptsDir, "user-password.sh")
		err := passwordScript(passwordScriptPath, configData)
		if err != nil {
			return fmt.Errorf("failed to write password script: %w", err)
		}
		configData.PasswordScript, err = filepath.Abs(passwordScriptPath)
		if err != nil {
			return fmt.Errorf("failed to get absolute path for password script: %w", err)
		}
	}

	// Render the Host Configuration
	templateName := filepath.Base(configPath)
	tmpl, err := template.New(templateName).Parse(templateContent)
	if err != nil {
		return fmt.Errorf("failed to parse template: %w", err)
	}
	out, err := os.Create(configPath)
	if err != nil {
		return fmt.Errorf("failed to create Host Configuration file: %w", err)
	}
	defer out.Close()
	return tmpl.Execute(out, configData)
}

// passwordScript generates and writes a shell script that sets the user password.
// The script uses chpasswd with the -e flag to accept pre-hashed passwords.
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
