package configuration

import (
	_ "embed"
	"fmt"
	"os"
	"path/filepath"
	"text/template"
)

const (
	configPath         = "/etc/trident/config.yaml"
	passwordScriptPath = "/etc/trident/scripts/user-password.sh"
)

//go:embed template/host-config.yaml.tmpl
var hostConfigTemplate string

// Creates Host Configuration using the template and adding the given user input.
func RenderTridentHostConfig(configData *TridentConfigData) error {
	passwordScriptPath, err := passwordScript(configData)
	if err != nil {
		return fmt.Errorf("failed to write password script: %w", err)
	}
	configData.PasswordScript = passwordScriptPath

	tmpl, err := template.New("host-config").Parse(hostConfigTemplate)
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

// Creates script to add user's password. Returns the script file path if successful.
func passwordScript(configData *TridentConfigData) (savedFile string, err error) {
	savedFile = ""
	script := fmt.Sprintf("echo '%s:%s' | chpasswd\n", configData.Username, configData.Password)
	dir := filepath.Dir(passwordScriptPath)
	if err = os.MkdirAll(dir, 0700); err != nil {
		return
	}
	if err = os.WriteFile(passwordScriptPath, []byte(script), 0700); err != nil {
		return
	}
	savedFile = passwordScriptPath
	return
}
