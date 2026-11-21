package virtdeploy

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

type filesystemPath struct {
	path      string
	info      os.FileInfo
	lexical   string
	traversed []string
}

type resourceFilesystem interface {
	resolve(string) (filesystemPath, error)
	readDir(string) ([]os.DirEntry, error)
}

type osResourceFilesystem struct{}

func (osResourceFilesystem) resolve(value string) (filesystemPath, error) {
	resolved, err := resolveFilesystemPath(value)
	if err != nil {
		return filesystemPath{}, err
	}
	resolved.lexical = value
	resolved.traversed, err = traceFilesystemPath(value)
	if err != nil {
		return filesystemPath{}, fmt.Errorf("trace storage path %q: %w", value, err)
	}
	return resolved, nil
}

func resolveFilesystemPath(value string) (filesystemPath, error) {
	if !filepath.IsAbs(value) {
		return filesystemPath{}, fmt.Errorf("cannot resolve non-absolute storage path %q", value)
	}
	original := value
	var missing []string
	for {
		resolved, err := filepath.EvalSymlinks(value)
		if err == nil {
			info, err := os.Stat(resolved)
			if err != nil {
				return filesystemPath{}, fmt.Errorf("stat storage path %q: %w", original, err)
			}
			if len(missing) != 0 {
				if !info.IsDir() {
					return filesystemPath{}, fmt.Errorf("storage path %q has a non-directory ancestor", original)
				}
				return filesystemPath{path: filepath.Join(append([]string{resolved}, missing...)...)}, nil
			}
			return filesystemPath{path: resolved, info: info}, nil
		}
		if !os.IsNotExist(err) {
			return filesystemPath{}, fmt.Errorf("resolve storage path %q: %w", original, err)
		}
		// ENOENT can mean a dangling symlink, not an absent directory/file.
		// Only project missing suffixes from an established existing ancestor.
		if _, statErr := os.Lstat(strings.TrimRight(value, string(os.PathSeparator))); !os.IsNotExist(statErr) {
			return filesystemPath{}, fmt.Errorf("cannot establish storage path %q identity: %w", original, err)
		}
		parent, base := filepath.Split(strings.TrimRight(value, string(os.PathSeparator)))
		if base == ".." || base == "" {
			return filesystemPath{}, fmt.Errorf("cannot establish storage path %q identity", original)
		}
		missing = append([]string{base}, missing...)
		value = parent
	}
}

// Record symlink entries as well as their referents. Unlinking an intermediate
// entry can break a retained path even when its final referent is outside it.
func traceFilesystemPath(value string) ([]string, error) {
	separator := string(os.PathSeparator)
	current := separator
	pending := strings.Split(value, separator)
	var traversed []string
	links := 0
	for len(pending) != 0 {
		component := pending[0]
		pending = pending[1:]
		if component == "" || component == "." {
			continue
		}
		if component == ".." {
			current = filepath.Dir(current)
			continue
		}
		current = filepath.Join(current, component)
		traversed = append(traversed, current)
		info, err := os.Lstat(current)
		if os.IsNotExist(err) {
			// resolveFilesystemPath already verified the missing suffix.
			continue
		}
		if err != nil {
			return nil, err
		}
		if info.Mode()&os.ModeSymlink == 0 {
			continue
		}
		if _, err := filepath.EvalSymlinks(current); err != nil {
			return nil, err
		}
		target, err := os.Readlink(current)
		if err != nil {
			return nil, err
		}
		links++
		if links > 255 {
			return nil, fmt.Errorf("too many symlinks in %q", value)
		}
		if filepath.IsAbs(target) {
			current = separator
		} else {
			current = filepath.Dir(current)
		}
		pending = append(strings.Split(target, separator), pending...)
	}
	return traversed, nil
}

func (osResourceFilesystem) readDir(value string) ([]os.DirEntry, error) {
	return os.ReadDir(value)
}

func (rc *virtDeployResourceConfig) verifyReplacementDirectories(files resourceFilesystem, activePools map[string]bool) error {
	for name, directory := range map[string]string{
		rc.namespace.storagePoolName(): rc.namespace.storagePoolPath(),
		rc.namespace.nvramPoolName():   rc.namespace.nvramPoolPath(),
	} {
		if activePools[name] {
			continue
		}
		entries, err := files.readDir(directory)
		if os.IsNotExist(err) {
			continue
		}
		if err != nil {
			return fmt.Errorf("inspect storage directory %s: %w", directory, err)
		}
		if len(entries) != 0 {
			return fmt.Errorf("unverified files remain in storage directory %s; refusing replacement", directory)
		}
	}
	return nil
}
