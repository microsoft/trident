package virtdeploy

import (
	"os"
	"path/filepath"
	"testing"
)

func TestFilesystemIdentityResolution(t *testing.T) {
	root := t.TempDir()
	actual := filepath.Join(root, "actual")
	if err := os.MkdirAll(filepath.Join(actual, "nested"), 0755); err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(actual, "disk.qcow2")
	if err := os.WriteFile(file, []byte("preserve"), 0600); err != nil {
		t.Fatal(err)
	}
	for name, target := range map[string]string{
		"alias":    filepath.Join(actual, "nested"),
		"dangling": filepath.Join(root, "missing"),
		"loop":     filepath.Join(root, "loop"),
		"hardlink": file,
	} {
		link := filepath.Join(root, name)
		var err error
		if name == "hardlink" {
			err = os.Link(target, link)
		} else {
			err = os.Symlink(target, link)
		}
		if err != nil {
			t.Fatal(err)
		}
	}
	files := osResourceFilesystem{}
	target, err := files.resolve(file)
	if err != nil {
		t.Fatal(err)
	}
	for _, test := range []struct {
		name string
		path string
		want string
	}{
		{"existing", file, file},
		{"symlink before dot-dot", root + "/alias/../disk.qcow2", file},
		{"missing suffix", root + "/alias/missing/image", filepath.Join(actual, "nested", "missing", "image")},
		{"missing normalized suffix", root + "/absent/./image", filepath.Join(root, "absent", "image")},
		{"dangling symlink", root + "/dangling", ""},
		{"dangling ancestor", root + "/dangling/image", ""},
		{"symlink loop", root + "/loop", ""},
		{"non-directory ancestor", file + "/image", ""},
		{"unknown parent traversal", root + "/absent/../image", ""},
		{"relative path", "disk.qcow2", ""},
	} {
		t.Run(test.name, func(t *testing.T) {
			resolved, err := files.resolve(test.path)
			if test.want == "" {
				if err == nil {
					t.Fatal("uncertain filesystem identity must fail")
				}
				return
			}
			if err != nil || resolved.path != test.want {
				t.Fatalf("resolved %q, want %q: %v", resolved.path, test.want, err)
			}
		})
	}
	targets := resourceTargets{paths: []filesystemPath{target}}
	overlaps, err := targets.overlapsPath(filepath.Join(root, "hardlink"), files)
	if err != nil || !overlaps {
		t.Fatalf("same-inode reference must be detected: overlaps=%v, err=%v", overlaps, err)
	}
}

func TestPreflightFailsClosedOnUnreadableFilesystem(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("permission denial requires an unprivileged user")
	}
	for _, replacement := range []bool{false, true} {
		t.Run(map[bool]string{false: "reference", true: "replacement directory"}[replacement], func(t *testing.T) {
			rc, client, files := filesystemPreflightConfig(t)
			directory := filepath.Join(files.root, "private")
			if replacement {
				directory = filepath.Join(files.root, "disks")
				client.pools = nil
			} else {
				if err := os.Mkdir(directory, 0700); err != nil {
					t.Fatal(err)
				}
				client.domains["other"] = `<domain><devices><disk type="file"><source file="` + directory + `/disk.qcow2"/></disk></devices></domain>`
			}
			if err := os.Chmod(directory, 0); err != nil {
				t.Fatal(err)
			}
			defer func() {
				if err := os.Chmod(directory, 0700); err != nil {
					t.Error(err)
				}
			}()
			var err error
			if replacement {
				_, err = rc.construct()
			} else {
				err = rc.cleanup()
			}
			if err == nil || len(client.deleted) != 0 {
				t.Fatalf("unreadable identity/directory must abort without mutation: %v, %v", err, client.deleted)
			}
		})
	}
}
