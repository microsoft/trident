# Share config with `$include`

Use `$include` to splice a shared YAML file into a field value.

Create a shared storage layout:

```bash
mkdir -p gizmo/layouts/storage
cat > gizmo/layouts/storage/lite.yaml <<'EOF'
bootType: efi
disks:
  - partitionTableType: gpt
    partitions:
      - id: esp
        type: esp
        size: 8M
      - id: rootfs
        size: grow
filesystems:
  - deviceId: esp
    type: fat32
    mountPoint:
      path: /boot/efi
  - deviceId: rootfs
    type: ext4
    mountPoint:
      path: /
EOF
```

Include it from a fragment:

```yaml
# gizmo/by-edition/lite.yaml
config:
  storage:
    $include: layouts/storage/lite.yaml
```

The included file is the value of `storage`; do not repeat `storage:` inside the included file. `$include` may also appear as a list item; if the included file is a list, its elements are spliced into the surrounding list.
